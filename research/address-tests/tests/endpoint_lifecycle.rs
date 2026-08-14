//! Endpoint lifecycle accounting: every endpoint handle created (by claim
//! or by a resolve snapshot clone) must be dropped exactly once — no
//! leaks, no double drops — and the number of LIVE handles must track the
//! model at every step.
//!
//! Excluded from Miri (case counts are a native-speed workload).
#![cfg(not(miri))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bombay_address::{AddressInUse, AddressSpace, Lease};
use proptest::prelude::*;

/// Shared creation/drop counters for a whole history run.
#[derive(Default)]
struct Accounting {
    created: AtomicUsize,
    dropped: AtomicUsize,
}

/// An endpoint whose clones and drops are counted.
struct Counted {
    value: u64,
    accounting: Arc<Accounting>,
}

impl Counted {
    fn new(value: u64, accounting: &Arc<Accounting>) -> Self {
        accounting.created.fetch_add(1, Ordering::SeqCst);
        Self {
            value,
            accounting: Arc::clone(accounting),
        }
    }
}

impl Clone for Counted {
    fn clone(&self) -> Self {
        self.accounting.created.fetch_add(1, Ordering::SeqCst);
        Self {
            value: self.value,
            accounting: Arc::clone(&self.accounting),
        }
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.accounting.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

fn live(accounting: &Accounting) -> usize {
    let created = accounting.created.load(Ordering::SeqCst);
    let dropped = accounting.dropped.load(Ordering::SeqCst);
    created - dropped
}

fn run(ops: &[(u8, u8)]) {
    let space = AddressSpace::<u64, Counted>::new();
    let accounting = Arc::new(Accounting::default());
    let mut model: std::collections::BTreeMap<u64, u64> = Default::default();
    let mut leases: std::collections::BTreeMap<u64, Lease<u64, Counted>> = Default::default();
    let mut endpoint = 0_u64;
    for (step, (address, op)) in ops.iter().enumerate() {
        let address = u64::from(address % 8);
        match op % 3 {
            0 => {
                endpoint += 1;
                match space.claim(address, Counted::new(endpoint, &accounting)) {
                    Ok(lease) => {
                        assert!(model.insert(address, endpoint).is_none(), "step {step}");
                        leases.insert(address, lease);
                    }
                    Err(AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(model.contains_key(&address), "step {step}");
                    }
                }
            }
            1 => {
                let resolved = space.resolve(&address);
                assert_eq!(
                    resolved.as_ref().map(|c| c.value),
                    model.get(&address).copied(),
                    "step {step}: resolve diverged"
                );
                // The snapshot drops at the end of this statement; both
                // its creation and drop are accounted.
            }
            _ => {
                if let Some(lease) = leases.remove(&address) {
                    lease.release();
                    model.remove(&address);
                }
                assert_eq!(space.resolve(&address).map(|c| c.value), None, "step {step}");
            }
        }
        assert_eq!(space.len(), model.len(), "step {step}");
        // Live handles: one per live registration (transient resolve
        // snapshots are already dropped by this point).
        assert_eq!(
            live(&accounting),
            model.len(),
            "step {step}: endpoint handles leaked or double-dropped"
        );
    }
    for (_, lease) in leases {
        lease.release();
    }
    assert!(space.is_empty());
    assert_eq!(live(&accounting), 0, "handles leaked after drain");
    assert_eq!(
        accounting.created.load(Ordering::SeqCst),
        accounting.dropped.load(Ordering::SeqCst),
        "every created handle must be dropped exactly once"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn endpoint_handles_balance_exactly(ops in prop::collection::vec((any::<u8>(), any::<u8>()), 1..120)) {
        run(&ops);
    }
}

/// Deterministic boundary addresses: the extreme keys of the u64 domain
/// must coexist, resolve, and release exactly (hasher boundary behavior).
#[test]
fn boundary_addresses_coexist_exactly() {
    let space = AddressSpace::new();
    let addresses = [0_u64, 1, u64::MAX - 1, u64::MAX];
    let mut leases = Vec::new();
    for (i, &address) in addresses.iter().enumerate() {
        leases.push(space.claim(address, i as u64).unwrap());
    }
    assert_eq!(space.len(), 4);
    for (i, &address) in addresses.iter().enumerate() {
        assert_eq!(space.resolve(&address).as_deref().copied(), Some(i as u64));
    }
    // Duplicate claims on the boundary keys are rejected with the exact
    // address returned.
    for &address in &addresses {
        assert!(matches!(
            space.claim(address, 99),
            Err(AddressInUse(a)) if a == address
        ));
    }
    for lease in leases {
        lease.release();
    }
    assert!(space.is_empty());
}

/// Deterministic lifecycle check on the replacement path: releasing and
/// reclaiming one address must account both endpoints exactly.
#[test]
fn replacement_accounts_both_generations() {
    let space = AddressSpace::new();
    let accounting = Arc::new(Accounting::default());
    let first = space.claim(0_u64, Counted::new(1, &accounting)).unwrap();
    let snapshot = space.resolve(&0).unwrap();
    assert_eq!(live(&accounting), 2); // registration + snapshot
    drop(snapshot);
    assert_eq!(live(&accounting), 1);
    first.release();
    assert_eq!(live(&accounting), 0);
    let second = space.claim(0_u64, Counted::new(2, &accounting)).unwrap();
    assert_eq!(live(&accounting), 1);
    second.release();
    assert_eq!(live(&accounting), 0);
    assert_eq!(accounting.created.load(Ordering::SeqCst), 3);
    assert_eq!(accounting.dropped.load(Ordering::SeqCst), 3);
}
