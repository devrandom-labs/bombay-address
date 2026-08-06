//! AddressSpace lifetime: dropping the `AddressSpace` handle must not
//! affect registrations — leases and clones share `Arc<Inner>` ownership.
//! These paths pin the ownership semantics: a lease outliving every space
//! handle still releases its exact registration, and resolved snapshots
//! outlive the space they came from.

use addresspass::AddressSpace;

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
