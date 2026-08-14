use bombay_address::{AddressInUse, AddressSpace};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct ReentrantDrop {
    space: AddressSpace<u64, Self>,
    dropped: Arc<AtomicBool>,
}

impl Clone for ReentrantDrop {
    fn clone(&self) -> Self {
        Self {
            space: self.space.clone(),
            dropped: Arc::clone(&self.dropped),
        }
    }
}

impl Drop for ReentrantDrop {
    fn drop(&mut self) {
        assert!(self.space.resolve(&1).is_none());
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[test]
fn final_snapshot_drop_can_reenter_the_same_space() {
    let space = AddressSpace::new();
    let dropped = Arc::new(AtomicBool::new(false));
    let lease = space
        .claim(
            1,
            ReentrantDrop {
                space: space.clone(),
                dropped: Arc::clone(&dropped),
            },
        )
        .unwrap();
    let snapshot = space.resolve(&1).unwrap();

    drop(lease);
    assert!(dropped.load(Ordering::SeqCst));
    dropped.store(false, Ordering::SeqCst);
    drop(snapshot);
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn duplicate_claim_preserves_the_live_endpoint() {
    let space = AddressSpace::new();
    let _lease = space.claim(1_u64, 10_u64).unwrap();
    assert!(matches!(space.claim(1, 20), Err(AddressInUse(1))));
    assert_eq!(space.resolve(&1).as_deref().copied(), Some(10));
}

#[test]
fn cloned_spaces_share_registration_state() {
    let space = AddressSpace::new();
    let peer = space.clone();
    let lease = space.claim(1_u64, 10_u64).unwrap();
    assert_eq!(peer.resolve(&1).as_deref().copied(), Some(10));
    lease.release();
    assert_eq!(peer.resolve(&1), None);
}
