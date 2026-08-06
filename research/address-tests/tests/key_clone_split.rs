//! FINDING-005 probe: `claim` splits the key identity.
//!
//! Production `claim` clones the address BEFORE the write guard (to keep
//! caller `Clone` code out of the lock — by design), duplicate-checks the
//! ORIGINAL, inserts the CLONE, and hands the ORIGINAL to the `Lease`.
//! This assumes `Clone` is identity-preserving — an assumption NOT
//! licensed by the `A: Eq + Hash + Clone` bounds.
//!
//! A key whose `Clone` mutates the original through interior mutability
//! satisfies the `std::collections::HashMap` key contract at every call
//! site (no key is ever mutated while stored in the map), yet
//! desynchronizes the stored clone from the lease's original:
//!
//! - `claim` returns `Ok`, but `resolve(lease.address())` is `None`
//!   immediately — the registration was stored under the clone's
//!   identity, not the claimed address's;
//! - `lease.release()` silently no-ops (the generation gate correctly
//!   refuses to remove the OTHER registration now occupying the
//!   original's identity), while marking the lease released;
//! - the clone-keyed entry leaks for the life of the space.
//!
//! The active assertions express the correct behavior: a successful claim
//! is immediately resolvable through the lease's address, and release
//! frees it. They currently fail.
#![cfg(not(miri))]

use std::cell::Cell;
use std::hash::{Hash, Hasher};

use bombay_address::AddressSpace;

/// A key whose `Clone` bumps the ORIGINAL's id through interior
/// mutability; the clone keeps the pre-clone id. `Hash`/`Eq` are always
/// consistent with each other for any value at any instant, and no stored
/// key is ever mutated — the standard `HashMap` key contract holds
/// throughout.
#[derive(Debug)]
struct SplitOnClone {
    id: Cell<u64>,
}

impl Clone for SplitOnClone {
    fn clone(&self) -> Self {
        let clone = Self {
            id: Cell::new(self.id.get()),
        };
        self.id.set(self.id.get() + 1); // original desyncs from the clone
        clone
    }
}

impl PartialEq for SplitOnClone {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for SplitOnClone {}

impl Hash for SplitOnClone {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.id.get());
    }
}

impl SplitOnClone {
    fn new(id: u64) -> Self {
        Self { id: Cell::new(id) }
    }
}

#[test]
#[ignore = "FINDING-005: claim duplicate-checks the original key but stores the clone; a legal non-identity Clone makes the claim unresolvable and the release a silent no-op (entry leaks)"]
fn successful_claim_is_immediately_resolvable_and_releasable() {
    let space = AddressSpace::new();
    let lease = space.claim(SplitOnClone::new(1), 42_u64).unwrap();

    // Correct behavior #1: the claimed address is live immediately.
    assert_eq!(
        space.resolve(lease.address()),
        Some(42),
        "claim returned Ok but resolve(lease.address()) is None: the \
         table stored the clone, the lease holds the desynced original \
         (FINDING-005)"
    );

    // Correct behavior #2: release frees the registration.
    lease.release();
    assert!(
        space.is_empty(),
        "release reported success but the entry leaked (FINDING-005)"
    );
}

/// Control: with identity `Clone` (the normal case) claim/resolve/release
/// are exact — the mechanism under test is the non-identity `Clone`, not
/// the test harness. This test stays ACTIVE and must always pass.
#[test]
fn identity_clone_control_claim_resolve_release_exact() {
    let space = AddressSpace::new();
    let lease = space.claim(1_u64, 42_u64).unwrap();
    assert_eq!(space.resolve(lease.address()), Some(42));
    lease.release();
    assert!(space.is_empty());
}

/// Pin the exact failure shape (as a documentation probe, ignored): the
/// claim leaks one entry and the lease's release no-ops.
#[test]
#[ignore = "FINDING-005 companion: documents the leak shape — len() stays 1 after the lease's release"]
fn finding_005_leak_shape() {
    let space = AddressSpace::new();
    let lease = space.claim(SplitOnClone::new(10), 1_u64).unwrap();
    assert_eq!(space.len(), 1);
    lease.release(); // silently no-ops: original's id no longer matches
    assert_eq!(
        space.len(),
        0,
        "the clone-keyed entry leaked: len() is still 1 after release"
    );
}

/// Severe variant of FINDING-005: exclusive ownership is broken.
///
/// `GainOnClone` inverts the desync: the CLONE gets id+1, the original
/// keeps its id. The first claim stores clone id 2 while the lease holds
/// id 1. A second claim of an Eq-EQUAL address (id 1) duplicate-checks
/// the original (id 1) — which misses, because the table holds id 2 —
/// and then inserts its clone (id 2), silently REPLACING the live entry.
#[derive(Debug)]
struct GainOnClone {
    id: Cell<u64>,
}

impl Clone for GainOnClone {
    fn clone(&self) -> Self {
        Self {
            id: Cell::new(self.id.get() + 1), // clone desyncs upward
        }
    }
}

impl PartialEq for GainOnClone {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for GainOnClone {}

impl Hash for GainOnClone {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.id.get());
    }
}

impl GainOnClone {
    fn new(id: u64) -> Self {
        Self { id: Cell::new(id) }
    }
}

/// Core invariant violation: a claim of an address that is Eq-equal to a
/// LIVE registration must be rejected with `AddressInUse`. With the split
/// key identity it succeeds — two leases for one logical address.
#[test]
#[ignore = "FINDING-005b: duplicate claim of an Eq-equal address succeeds when the first key's clone desynced; exclusive ownership is violated"]
fn duplicate_claim_of_equal_address_is_rejected() {
    use bombay_address::AddressInUse;

    let space = AddressSpace::new();
    let first = space.claim(GainOnClone::new(1), 10_u64).unwrap();
    // k2 == the originally claimed address (id 1): must be rejected.
    let second = space.claim(GainOnClone::new(1), 20_u64);
    assert!(
        matches!(second, Err(AddressInUse(_))),
        "duplicate claim of an Eq-equal live address succeeded: \
         exclusive ownership violated (FINDING-005b)"
    );
    first.release();
}

/// Fallout: the overwrite drops the FIRST endpoint inline (inside
/// `HashMap::insert`, under the write guard) while its lease is still
/// live — an endpoint must only be dropped when its own registration is
/// released.
#[test]
#[ignore = "FINDING-005c: the silently replaced endpoint is dropped under the write guard while its lease is still live"]
fn live_endpoint_is_not_dropped_before_its_release() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let drops = Arc::new(AtomicUsize::new(0));
    struct Counted(Arc<AtomicUsize>);
    impl Clone for Counted {
        fn clone(&self) -> Self {
            Self(Arc::clone(&self.0))
        }
    }
    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let space = AddressSpace::new();
    let first = space
        .claim(GainOnClone::new(1), Counted(Arc::clone(&drops)))
        .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    // The overwrite (FINDING-005b) drops `first`'s endpoint inline.
    let _second = space.claim(GainOnClone::new(1), Counted(Arc::clone(&drops)));
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "endpoint dropped while its lease was still live (dropped inline \
         by HashMap::insert under the write guard) (FINDING-005c)"
    );
    first.release();
}
