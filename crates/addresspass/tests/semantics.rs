use addresspass::{AddressInUse, AddressSpace};

#[test]
fn duplicate_claim_preserves_the_live_endpoint() {
    let space = AddressSpace::new();
    let _lease = space.claim(1_u64, 10_u64).unwrap();
    assert!(matches!(space.claim(1, 20), Err(AddressInUse(1))));
    assert_eq!(space.resolve(&1), Some(10));
}

#[test]
fn cloned_spaces_share_registration_state() {
    let space = AddressSpace::new();
    let peer = space.clone();
    let lease = space.claim(1_u64, 10_u64).unwrap();
    assert_eq!(peer.resolve(&1), Some(10));
    lease.release();
    assert_eq!(peer.resolve(&1), None);
}
