//! Explicit reclamation contract tests for `AddressSpace::shrink_to_fit`.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use bombay_address::{AddressInUse, AddressSpace};

/// A new, never-used space must tolerate `shrink_to_fit`.
#[test]
fn shrink_to_fit_on_new_space_is_safe() {
    let space = AddressSpace::<u64, u64>::new();
    space.shrink_to_fit();
    assert!(space.is_empty());
    let lease = space.claim(1, 10).unwrap();
    assert_eq!(space.resolve(&1).as_deref().copied(), Some(10));
    lease.release();
}

/// `shrink_to_fit` on a space that grew then drained is idempotent
/// and keeps the space usable.
#[test]
fn shrink_to_fit_on_empty_used_space_is_idempotent() {
    let space = AddressSpace::<u64, u64>::new();
    let lease = space.claim(1, 10).unwrap();
    lease.release();
    assert!(space.is_empty());
    space.shrink_to_fit();
    space.shrink_to_fit();
    assert!(space.is_empty());
    let re = space.claim(1, 20).unwrap();
    assert_eq!(space.resolve(&1).as_deref().copied(), Some(20));
    re.release();
}

/// Every live registration, its endpoint value, and its generation
/// survive `shrink_to_fit` unchanged.
#[test]
fn shrink_to_fit_preserves_live_registrations() {
    let space = AddressSpace::<u64, u64>::new();
    let mut leases = Vec::new();
    for addr in 0..1_000_u64 {
        let value = addr.wrapping_mul(3);
        leases.push(space.claim(addr, value).unwrap());
    }
    assert_eq!(space.len(), 1_000);
    space.shrink_to_fit();
    assert_eq!(space.len(), 1_000);
    for addr in 0..1_000_u64 {
        assert_eq!(
            space.resolve(&addr).as_deref().copied(),
            Some(addr.wrapping_mul(3))
        );
    }
    for lease in leases {
        lease.release();
    }
    assert!(space.is_empty());
}

/// After shrinking, each surviving registration releases with its
/// exact generation — the generation gate must still match.
#[test]
fn shrink_to_fit_preserves_exact_generation_release() {
    let space = AddressSpace::<u64, u64>::new();
    for round in 1..=100_u64 {
        let lease = space.claim(7, round).unwrap();
        space.shrink_to_fit();
        assert_eq!(space.resolve(&7).as_deref().copied(), Some(round));
        lease.release();
        assert_eq!(space.resolve(&7), None);
    }
}

/// Resolved snapshots taken before `shrink_to_fit` keep their values
/// and the space stays consistent afterwards.
#[test]
fn shrink_to_fit_preserves_resolved_snapshots() {
    let space = AddressSpace::<u64, u64>::new();
    let lease = space.claim(0, 42).unwrap();
    let snapshot = space.resolve(&0).unwrap();
    assert_eq!(snapshot, 42);
    space.shrink_to_fit();
    assert_eq!(snapshot, 42);
    lease.release();
    assert_eq!(snapshot, 42); // snapshot stays alive
    assert!(space.is_empty());
}

/// A Counted endpoint type for verifying no-drop contracts.
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

/// `shrink_to_fit` must never drop or replace live endpoints.
#[test]
fn shrink_to_fit_does_not_drop_live_endpoints() {
    let drops = Arc::new(AtomicUsize::new(0));
    let space = AddressSpace::<u64, Counted>::new();
    let lease = space.claim(0, Counted(Arc::clone(&drops))).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    space.shrink_to_fit();
    assert_eq!(
        drops.load(Ordering::SeqCst),
        0,
        "shrink_to_fit dropped an endpoint"
    );
    assert!(space.resolve(&0).is_some());
    lease.release();
    assert!(drops.load(Ordering::SeqCst) >= 1);
    assert!(space.is_empty());
}

/// `claim`, `resolve`, and `release` continue to work exactly after
/// shrinking.
#[test]
fn claim_resolve_release_still_work_after_shrink() {
    let space = AddressSpace::<u64, u64>::new();
    let mut leases = Vec::new();
    for addr in 0..500_u64 {
        leases.push(space.claim(addr, addr).unwrap());
    }
    space.shrink_to_fit();
    for addr in 500..1_000_u64 {
        leases.push(space.claim(addr, addr).unwrap());
    }
    space.shrink_to_fit();
    assert_eq!(space.len(), 1_000);
    let rejected = space.claim(0, 999);
    assert!(matches!(rejected, Err(AddressInUse(0))));
    for addr in 0..1_000_u64 {
        assert_eq!(
            space.resolve(&addr).as_deref().copied(),
            Some(addr),
            "lost {addr} after shrink"
        );
    }
    for lease in leases.drain(..500) {
        drop(lease);
    }
    space.shrink_to_fit();
    assert_eq!(space.len(), 500);
    for addr in 500..1_000_u64 {
        assert_eq!(space.resolve(&addr).as_deref().copied(), Some(addr));
    }
    for lease in leases {
        drop(lease);
    }
    assert!(space.is_empty());
}

/// Repeated grow/drain/shrink cycles do not corrupt the space.
#[test]
fn repeated_grow_drain_shrink_cycles_reclaim_memory() {
    let space = AddressSpace::<u64, u64>::new();
    #[allow(
        clippy::cast_possible_truncation,
        reason = "BURST=50000 fits in usize on 64-bit"
    )]
    let burst: usize = 50_000;
    for _cycle in 0..3 {
        let mut leases: Vec<_> = (0..burst)
            .map(|addr| {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "addr from 0..burst fits u64"
                )]
                let addr = addr as u64;
                space.claim(addr, addr).unwrap()
            })
            .collect();
        assert_eq!(space.len(), burst);
        for lease in leases.drain(..) {
            lease.release();
        }
        assert!(space.is_empty());
        space.shrink_to_fit();
        let re = space.claim(1, 99).unwrap();
        assert_eq!(space.resolve(&1).as_deref().copied(), Some(99));
        re.release();
    }
}
