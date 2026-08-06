//! AddressSpace lifetime: dropping the `AddressSpace` handle must not
//! affect registrations — leases and clones share `Arc<Inner>` ownership.
//! These paths pin the ownership semantics: a lease outliving every space
//! handle still releases its exact registration, and resolved snapshots
//! outlive the space they came from.

use bombay_address::AddressSpace;

#[test]
fn lease_outlives_all_space_handles_and_still_releases_exactly() {
    let peer;
    let lease;
    {
        let space = AddressSpace::new();
        peer = space.clone();
        lease = space.claim(1_u64, 10_u64).unwrap();
        // `space` drops here; `peer` and `lease` keep Inner alive.
    }
    assert_eq!(peer.resolve(&1), Some(10));
    lease.release();
    assert_eq!(peer.resolve(&1), None);
    assert!(peer.is_empty());
    // The peer remains fully operational after the original handle is
    // gone and the lease cycle completed.
    let again = peer.claim(1_u64, 20_u64).unwrap();
    assert_eq!(peer.resolve(&1), Some(20));
    again.release();
    assert!(peer.is_empty());
}

#[test]
fn resolved_snapshot_outlives_the_space_and_the_registration() {
    let snapshot = {
        let space = AddressSpace::new();
        let lease = space.claim(7_u64, String::from("persistent")).unwrap();
        let snapshot = space.resolve(&7).unwrap();
        lease.release();
        drop(space);
        snapshot
    };
    assert_eq!(snapshot, "persistent");
}

#[test]
fn lease_dropped_after_all_space_handles_releases_into_shared_state() {
    let space = AddressSpace::new();
    let peer = space.clone();
    let lease = space.claim(3_u64, 30_u64).unwrap();
    drop(space);
    drop(peer);
    // No AddressSpace handle remains; dropping the lease must still run
    // the release path against the shared state without a handle.
    lease.release();
    // Fresh observation is impossible without a handle, so reconstruct
    // the scenario: this must not have panicked or deadlocked, and a new
    // space is independent.
    let fresh = AddressSpace::<u64, u64>::new();
    assert!(fresh.is_empty());
}

#[test]
fn default_constructed_space_behaves_like_new() {
    let space = AddressSpace::<u64, u64>::default();
    assert!(space.is_empty());
    let lease = space.claim(0, 1).unwrap();
    assert_eq!(space.len(), 1);
    assert_eq!(space.resolve(&0), Some(1));
    lease.release();
    assert!(space.is_empty());
}

#[test]
fn lease_address_accessor_returns_the_owned_address() {
    let space = AddressSpace::new();
    let lease = space.claim(42_u64, 1_u64).unwrap();
    assert_eq!(lease.address(), &42);
    lease.release();
}

#[test]
fn independent_spaces_share_no_state() {
    let first = AddressSpace::new();
    let second = AddressSpace::new();
    let lease_a = first.claim(1_u64, 10_u64).unwrap();
    let lease_b = second.claim(1_u64, 20_u64).unwrap(); // same address: independent
    assert_eq!(first.resolve(&1), Some(10));
    assert_eq!(second.resolve(&1), Some(20));
    lease_a.release();
    assert_eq!(first.resolve(&1), None);
    assert_eq!(second.resolve(&1), Some(20)); // untouched by peer release
    assert_eq!(second.len(), 1);
    lease_b.release();
    assert!(first.is_empty() && second.is_empty());
}

/// Lease migration: a lease claimed on one thread can be moved to and
/// released on another (the actor-pattern handoff). Small enough for the
/// Miri lane.
#[test]
fn lease_migrates_across_threads_and_releases_exactly() {
    let space = AddressSpace::new();
    let lease = space.claim(9_u64, 90_u64).unwrap();
    let peer = space.clone();
    let handle = std::thread::spawn(move || {
        assert_eq!(*lease.address(), 9);
        lease.release();
    });
    handle.join().unwrap();
    assert_eq!(space.resolve(&9), None);
    assert!(peer.is_empty());
}
