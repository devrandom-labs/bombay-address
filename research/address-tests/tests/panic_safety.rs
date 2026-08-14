//! Panic safety: caller code (`Hash`, `Eq`, endpoint `Clone`) may panic;
//! the address space must stay consistent and usable afterwards.
//!
//! FINDING-003 lives here: `release` is not panic-atomic.

use std::cell::Cell;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bombay_address::AddressSpace;

/// A key whose `Hash` panics on the `panic_on`-th global hash call. The
/// counter is shared across clones (claim hashes twice — `get` then
/// `insert` — and release hashes once more, so the call at which the
/// panic lands selects which operation unwinds).
#[derive(Clone)]
struct SometimesPanic {
    id: u64,
    calls: Arc<AtomicUsize>,
    panic_on: usize,
}

impl SometimesPanic {
    fn good(id: u64) -> Self {
        Self {
            id,
            calls: Arc::new(AtomicUsize::new(0)),
            panic_on: usize::MAX,
        }
    }
}

impl PartialEq for SometimesPanic {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for SometimesPanic {}

impl Hash for SometimesPanic {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.panic_on {
            panic!("injected hash panic on call {call}");
        }
        state.write_u64(self.id);
    }
}

impl fmt::Debug for SometimesPanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SometimesPanic({})", self.id)
    }
}

/// A panic inside `Hash` during `claim`'s duplicate check unwinds through
/// the write guard. Afterwards the space must be fully functional.
///
/// Note: hashbrown's `get` on an EMPTY table returns without hashing, so
/// the space is pre-seeded to make the duplicate check hash at all.
#[test]
fn hash_panic_during_claim_leaves_space_consistent() {
    let space = AddressSpace::new();
    let seed = space.claim(SometimesPanic::good(999), 0_u64).unwrap();
    let panic_key = SometimesPanic {
        panic_on: 1, // panic on the first hash of this claim (the get)
        ..SometimesPanic::good(1)
    };
    let result = catch_unwind(AssertUnwindSafe(|| space.claim(panic_key, 10_u64)));
    assert!(result.is_err(), "injected panic must propagate");
    // The space is unharmed: claim, resolve, release all work.
    let lease = space.claim(SometimesPanic::good(1), 20_u64).unwrap();
    assert_eq!(
        space.resolve(&SometimesPanic::good(1)).as_deref().copied(),
        Some(20)
    );
    assert_eq!(space.len(), 2);
    lease.release();
    seed.release();
    assert!(space.is_empty());
}

/// A panic inside `Hash` during `claim`'s INSERT (after the duplicate
/// check passed) must also leave the space consistent — and must not have
/// consumed an observable registration slot.
#[test]
fn hash_panic_during_claim_insert_leaves_no_ghost_registration() {
    let space = AddressSpace::new();
    // Pre-seed so `get` hashes (empty tables return early): then the
    // duplicate check hashes (call 1) and the insert hashes (call 2).
    let seed = space.claim(SometimesPanic::good(999), 0_u64).unwrap();
    let key = SometimesPanic {
        panic_on: 2, // panic on the insert's hash
        ..SometimesPanic::good(7)
    };
    let result = catch_unwind(AssertUnwindSafe(|| space.claim(key, 10_u64)));
    assert!(result.is_err());
    // No ghost: the address is free, re-claim works.
    let lease = space.claim(SometimesPanic::good(7), 30_u64).unwrap();
    assert_eq!(
        space.resolve(&SometimesPanic::good(7)).as_deref().copied(),
        Some(30)
    );
    lease.release();
    seed.release();
    assert!(space.is_empty());
}

/// A panicking endpoint `Clone` during `resolve` must not corrupt or lock
/// the space.
#[test]
fn endpoint_clone_panic_during_resolve_leaves_space_consistent() {
    struct ClonePanic {
        calls: Cell<usize>,
    }
    impl Clone for ClonePanic {
        fn clone(&self) -> Self {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            if call == 1 {
                panic!("injected clone panic");
            }
            Self {
                calls: Cell::new(self.calls.get()),
            }
        }
    }

    let space = AddressSpace::new();
    let lease = space.claim(0_u64, ClonePanic { calls: Cell::new(0) }).unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| space.resolve(&0)));
    assert!(result.is_err(), "injected clone panic must propagate");
    // Second resolve succeeds (counter advanced past the panicking call).
    assert!(space.resolve(&0).is_some());
    lease.release();
    assert!(space.is_empty());
}

/// FINDING-003: `Lease::release` sets its `released` flag BEFORE the table
/// removal runs. If caller `Hash`/`Eq` panics during the removal, the
/// lease is consumed (its `Drop` early-returns on the flag) but the entry
/// was never removed: the address is permanently wedged — claimed forever,
/// with no handle able to release it.
///
/// The active assertion expresses the correct behavior: after a caught
/// release panic, the registration must be gone (the operation must be
/// panic-atomic). It currently fails: the entry leaks.
#[test]
#[ignore = "FINDING-003: release marks the lease released before removal; a panicking Hash during release wedges the address permanently"]
fn release_panic_does_not_wedge_the_address() {
    let space = AddressSpace::new();
    let key = SometimesPanic {
        panic_on: 3, // claim hashes twice; release's removal hashes 3rd
        ..SometimesPanic::good(3)
    };
    let lease = space.claim(key, 42_u64).unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| lease.release()));
    assert!(result.is_err(), "injected panic must propagate");

    // Correct behavior: the address is either released or still releasable.
    // The lease is consumed, so only "released" is acceptable.
    assert!(
        space.resolve(&SometimesPanic::good(3)).is_none(),
        "release panicked but the registration leaked: address is \
         permanently wedged (FINDING-003)"
    );
}

/// A re-entrant `Clone` on the ADDRESS runs before the write guard is
/// taken (production documents this), so a key `Clone` that claims and
/// releases another address in the same space must not deadlock.
#[test]
fn reentrant_address_clone_during_claim_does_not_deadlock() {
    struct ReentrantClone {
        id: u64,
        reenter: bool,
        space: Arc<AddressSpace<ReentrantClone, u64>>,
    }
    impl PartialEq for ReentrantClone {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }
    impl Eq for ReentrantClone {}
    impl Hash for ReentrantClone {
        fn hash<H: Hasher>(&self, state: &mut H) {
            state.write_u64(self.id);
        }
    }
    impl fmt::Debug for ReentrantClone {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "ReentrantClone({})", self.id)
        }
    }
    impl Clone for ReentrantClone {
        fn clone(&self) -> Self {
            if self.reenter {
                // Re-entrant claim/release cycle on a scratch address: no
                // lock is held yet, so this must complete. One-shot: the
                // scratch key does not itself re-enter (every claim clones
                // its key exactly once).
                let scratch = Self {
                    id: u64::MAX,
                    reenter: false,
                    space: Arc::clone(&self.space),
                };
                let lease = self.space.claim(scratch, 1).expect("scratch free");
                lease.release();
            }
            Self {
                id: self.id,
                reenter: self.reenter,
                space: Arc::clone(&self.space),
            }
        }
    }

    let space = Arc::new(AddressSpace::new());
    let key = ReentrantClone {
        id: 1,
        reenter: true,
        space: Arc::clone(&space),
    };
    let lease = space.claim(key, 9_u64).unwrap();
    assert_eq!(
        space.resolve(&ReentrantClone {
            id: 1,
            reenter: false,
            space: Arc::clone(&space)
        }).as_deref().copied(),
        Some(9)
    );
    lease.release();
    assert!(space.is_empty());
}

/// A key whose `Eq` panics while the shared `armed` flag is set (all
/// keys hash identically so the duplicate check and resolve always reach
/// `eq`).
#[derive(Clone)]
struct EqPanic {
    id: u64,
    armed: Arc<std::sync::atomic::AtomicBool>,
}

impl PartialEq for EqPanic {
    fn eq(&self, other: &Self) -> bool {
        if self.armed.load(Ordering::SeqCst) {
            panic!("injected eq panic");
        }
        self.id == other.id
    }
}
impl Eq for EqPanic {}

impl Hash for EqPanic {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(0); // constant hash: every probe reaches eq
    }
}

impl fmt::Debug for EqPanic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EqPanic({})", self.id)
    }
}

/// A panic inside `Eq` during `claim`'s duplicate check unwinds through
/// the write guard. Afterwards the space must be fully functional — the
/// Eq path (not just Hash) must be panic-safe on the claim path.
#[test]
fn eq_panic_during_claim_leaves_space_consistent() {
    let space = AddressSpace::new();
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seed = space
        .claim(
            EqPanic {
                id: 999,
                armed: Arc::clone(&armed),
            },
            0_u64,
        )
        .unwrap();
    armed.store(true, Ordering::SeqCst);
    let result = catch_unwind(AssertUnwindSafe(|| {
        space.claim(
            EqPanic {
                id: 1,
                armed: Arc::clone(&armed),
            },
            10_u64,
        )
    }));
    armed.store(false, Ordering::SeqCst);
    assert!(result.is_err(), "injected eq panic must propagate");
    // The space is unharmed: claim, resolve, release all work.
    let lease = space
        .claim(
            EqPanic {
                id: 1,
                armed: Arc::clone(&armed),
            },
            20_u64,
        )
        .unwrap();
    assert_eq!(
        space.resolve(&EqPanic {
            id: 1,
            armed: Arc::clone(&armed)
        }).as_deref().copied(),
        Some(20)
    );
    assert_eq!(space.len(), 2);
    lease.release();
    seed.release();
    assert!(space.is_empty());
}

/// A panic inside `Eq` during `resolve` leaves the space fully usable —
/// the Eq path (not just Hash and endpoint Clone) is panic-safe on the
/// resolve path.
#[test]
fn eq_panic_during_resolve_leaves_space_consistent() {
    let space = AddressSpace::new();
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lease = space
        .claim(
            EqPanic {
                id: 7,
                armed: Arc::clone(&armed),
            },
            70_u64,
        )
        .unwrap();
    armed.store(true, Ordering::SeqCst);
    let result = catch_unwind(AssertUnwindSafe(|| {
        space.resolve(&EqPanic {
            id: 7,
            armed: Arc::clone(&armed),
        })
    }));
    armed.store(false, Ordering::SeqCst);
    assert!(result.is_err(), "injected eq panic must propagate");
    // The space is unharmed and the lease still resolves.
    assert_eq!(
        space.resolve(&EqPanic {
            id: 7,
            armed: Arc::clone(&armed)
        }).as_deref().copied(),
        Some(70)
    );
    lease.release();
    assert!(space.is_empty());
}
