//! Panic-churn proptest: caller `Hash` panics are injected at RANDOM
//! positions of a history, not the hand-picked calls of the deterministic
//! panic tests. A shared "armed" flag makes the probe key panic on its
//! next hash; the space must stay fully consistent after every caught
//! panic, and the model must stay in lockstep (an armed claim is a clean
//! no-op).
//!
//! A permanent guard registration keeps the table non-empty, so the
//! duplicate-check `get` always hashes and an armed claim always panics
//! at the duplicate check (deterministic tests cover the insert-path
//! panic separately).
//!
//! Excluded from Miri (case counts are a native-speed workload).
#![cfg(not(miri))]

use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use addresspass::{AddressSpace, Lease};
use proptest::prelude::*;

/// A key whose `Hash` panics while the shared `armed` flag is set.
#[derive(Clone)]
struct PanicKey {
    id: u64,
    armed: Arc<AtomicBool>,
}

impl std::fmt::Debug for PanicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanicKey").field("id", &self.id).finish()
    }
}

impl PartialEq for PanicKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for PanicKey {}

impl Hash for PanicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.armed.load(Ordering::SeqCst) {
            panic!("injected hash panic");
        }
        state.write_u64(self.id);
    }
}

fn key(id: u64, armed: &Arc<AtomicBool>) -> PanicKey {
    PanicKey {
        id,
        armed: Arc::clone(armed),
    }
}

/// Replay a history where every `op % 7 == 0` claim is armed: the hash
/// panics at its duplicate check, the claim unwinds, and the model treats
/// it as a no-op. All other ops behave exactly as in the reference model.
fn run(ops: &[(u8, u8)]) {
    let space = AddressSpace::<PanicKey, u64>::new();
    let armed = Arc::new(AtomicBool::new(false));

    // Permanent guard: keeps the table non-empty so the duplicate check
    // always hashes (hashbrown skips hashing an empty table).
    let guard = space.claim(key(9_999, &armed), 0_u64).unwrap();

    let mut model: std::collections::BTreeMap<u64, u64> = Default::default();
    model.insert(9_999, 0);
    let mut leases: std::collections::BTreeMap<u64, Lease<PanicKey, u64>> = Default::default();

    for (step, (b0, b1)) in ops.iter().copied().enumerate() {
        let address = u64::from(b0 % 16);
        let want_panic = b1 % 7 == 0;
        match b0 % 4 {
            0 => {
                let endpoint = u64::from(b1) + 1;
                armed.store(want_panic, Ordering::SeqCst);
                let result = catch_unwind(AssertUnwindSafe(|| {
                    space.claim(key(address, &armed), endpoint)
                }));
                armed.store(false, Ordering::SeqCst);
                match result {
                    Ok(Ok(lease)) => {
                        assert!(
                            !want_panic,
                            "step {step}: armed claim must panic (guard keeps table non-empty)"
                        );
                        assert!(
                            model.insert(address, endpoint).is_none(),
                            "step {step}: SUT claimed an address the model owns"
                        );
                        leases.insert(address, lease);
                    }
                    Ok(Err(addresspass::AddressInUse(returned))) => {
                        assert_eq!(returned.id, address, "step {step}");
                        assert!(
                            model.contains_key(&address),
                            "step {step}: SUT rejected an address the model owns"
                        );
                        assert!(
                            !want_panic,
                            "step {step}: armed claim must panic, not reject"
                        );
                    }
                    Err(_) => {
                        // The armed claim panicked at its duplicate check:
                        // a clean no-op. The model must be unchanged and
                        // the address must still be free/owned as before.
                        assert!(want_panic, "step {step}: unarmed claim must not panic");
                        assert_eq!(
                            space.len(),
                            model.len(),
                            "step {step}: len diverged after caught panic"
                        );
                    }
                }
            }
            1 => {
                if let Some(lease) = leases.remove(&address) {
                    lease.release();
                    assert!(
                        model.remove(&address).is_some(),
                        "step {step}: SUT released an address the model does not own"
                    );
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&key(address, &armed)),
                    model.get(&address).copied(),
                    "step {step}: resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(
                    space.len(),
                    model.len(),
                    "step {step}: len diverged"
                );
            }
        }
        assert_eq!(space.len(), model.len(), "step {step}: len diverged");
    }

    for (address, lease) in leases {
        lease.release();
        model.remove(&address);
    }
    guard.release();
    model.remove(&9_999);
    assert!(space.is_empty() && model.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// Caught hash panics at random history positions leave the space
    /// fully consistent and the model in lockstep.
    #[test]
    fn caught_hash_panics_leave_space_consistent(
        ops in prop::collection::vec((any::<u8>(), any::<u8>()), 0..80)
    ) {
        run(&ops);
    }
}
