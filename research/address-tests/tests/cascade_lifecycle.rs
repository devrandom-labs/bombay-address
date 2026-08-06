//! Cascade lifecycle accounting: chain links are COUNTED endpoints; a
//! whole-chain cascade drops N links recursively through endpoint `Drop`s,
//! and every handle (claim payload + resolve snapshot clones) must be
//! dropped exactly once. Live-handle count must equal the model's
//! registration count at every step, and created == dropped at drain.
//!
//! The flat-endpoint lifecycle lane never exercises the recursive cascade
//! drop path; the cascade lane never accounts endpoints. This file closes
//! that gap.
//!
//! Excluded from Miri (case counts are a native-speed workload).
#![cfg(not(miri))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bombay_address::{AddressSpace, Lease};
use proptest::prelude::*;

/// Shared creation/drop counters for a whole history run.
#[derive(Default)]
struct Accounting {
    created: AtomicUsize,
    dropped: AtomicUsize,
}

/// A counted chain link: value for identity, plus the next lease down.
struct Link {
    value: u64,
    accounting: Arc<Accounting>,
    #[expect(
        dead_code,
        reason = "the field is exercised through drop (cascade recursion), never read"
    )]
    next: Option<Box<Lease<u64, Link>>>,
}

impl Link {
    /// Construct a link, counting the new endpoint handle (claim
    /// payloads are handles too — created here, dropped when the table
    /// removes the registration, or by `claim` on a failed claim).
    fn new(
        value: u64,
        accounting: Arc<Accounting>,
        next: Option<Box<Lease<u64, Link>>>,
    ) -> Self {
        accounting.created.fetch_add(1, Ordering::SeqCst);
        Self {
            value,
            accounting,
            next,
        }
    }
}

impl Clone for Link {
    fn clone(&self) -> Self {
        // A resolved snapshot is a counted handle but never owns the
        // chain below it.
        self.accounting.created.fetch_add(1, Ordering::SeqCst);
        Self {
            value: self.value,
            accounting: Arc::clone(&self.accounting),
            next: None,
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.accounting.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

fn live(accounting: &Accounting) -> usize {
    accounting.created.load(Ordering::SeqCst) - accounting.dropped.load(Ordering::SeqCst)
}

fn run(ops: &[(u8, u8)]) {
    let space = AddressSpace::<u64, Link>::new();
    let accounting = Arc::new(Accounting::default());
    let mut model: std::collections::BTreeMap<u64, u64> = Default::default();
    type ChainSlot = Option<(Lease<u64, Link>, Vec<u64>)>;
    let mut chains: [ChainSlot; 4] = [None, None, None, None];
    for (step, pair) in ops.iter().enumerate() {
        let slot = usize::from(pair.0 / 4) % 4;
        let base = (slot as u64) * 1_000;
        match pair.0 % 4 {
            0 => {
                let depth = 1 + u64::from(pair.1) % 8;
                let mut next: Option<Box<Lease<u64, Link>>> = None;
                let mut claimed: Vec<u64> = Vec::new();
                let mut aborted = false;
                for offset in (0..depth).rev() {
                    let address = base + offset;
                    match space.claim(
                        address,
                        Link::new(address, Arc::clone(&accounting), next.take()),
                    ) {
                        Ok(lease) => {
                            assert!(
                                model.insert(address, address).is_none(),
                                "step {step}: SUT claimed an address the model owns"
                            );
                            claimed.push(address);
                            next = Some(Box::new(lease));
                        }
                        Err(bombay_address::AddressInUse(returned)) => {
                            assert_eq!(returned, address, "step {step}");
                            assert!(
                                model.contains_key(&address),
                                "step {step}: SUT rejected an address the model owns"
                            );
                            aborted = true;
                            break;
                        }
                    }
                }
                if aborted {
                    // The rejected endpoint (holding the partial chain)
                    // was dropped by `claim`; those links dropped too.
                    for address in claimed {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: partial cascade released an address the model did not own"
                        );
                    }
                } else {
                    let top = *next.expect("depth >= 1");
                    chains[slot] = Some((top, claimed));
                }
            }
            1 => {
                if let Some((top, addresses)) = chains[slot].take() {
                    top.release();
                    for address in addresses {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: cascade released an address the model did not own"
                        );
                    }
                }
            }
            2 => {
                let address = base + u64::from(pair.1) % 8;
                // A resolve snapshot is a counted clone; dropping it must
                // balance its creation.
                let snapshot = space.resolve(&address);
                assert_eq!(
                    snapshot.as_ref().map(|l| l.value),
                    model.get(&address).copied(),
                    "step {step}: resolve({address}) diverged"
                );
                drop(snapshot);
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
        // Live endpoint handles must equal the model's registrations:
        // one claim payload per registration, plus any in-flight
        // snapshots (already dropped above, so none outstanding).
        assert_eq!(
            live(&accounting),
            model.len(),
            "step {step}: endpoint handles diverged from registrations"
        );
    }
    for slot in &mut chains {
        if let Some((top, addresses)) = slot.take() {
            top.release();
            for address in addresses {
                model.remove(&address);
            }
        }
    }
    assert!(space.is_empty() && model.is_empty());
    assert_eq!(
        live(&accounting),
        0,
        "every created endpoint handle must be dropped"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// Chain builds, whole-chain cascades, mid-build abort cascades, and
    /// resolves never leak or double-drop an endpoint handle, and the
    /// live-handle count tracks the registrations at every step.
    #[test]
    fn cascade_endpoint_handles_balance_exactly(
        ops in prop::collection::vec((any::<u8>(), any::<u8>()), 0..80)
    ) {
        run(&ops);
    }
}
