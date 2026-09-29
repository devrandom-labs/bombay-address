use bombay_address::{AddressInUse, AddressSpace, ClaimError};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn reservation_excludes_both_acquisition_paths_until_dropped() {
    let space = AddressSpace::<_, u64>::new();
    let address = String::from("worker");
    let reserved = space.try_reserve(address.clone()).unwrap();
    assert_eq!(reserved.address(), &address);
    assert_eq!(space.len(), 1);
    assert!(!space.is_empty());
    assert!(space.resolve(&address).is_none());
    assert!(
        matches!(space.try_reserve(address.clone()), Err(ClaimError::AddressInUse(a)) if a == address)
    );
    assert!(
        matches!(space.try_claim(address.clone(), 10), Err(ClaimError::AddressInUse(a)) if a == address)
    );
    assert!(matches!(space.claim(address.clone(), 10), Err(AddressInUse(a)) if a == address));
    let identity = reserved.registration_id();
    drop(reserved);
    assert!(space.is_empty());
    let claimed = space.try_claim(address.clone(), 20).unwrap();
    assert_ne!(claimed.registration_id(), identity);
    assert!(matches!(
        space.try_reserve(address),
        Err(ClaimError::AddressInUse(_))
    ));
}

#[test]
fn publish_preserves_identity_and_snapshot_semantics() {
    let space = AddressSpace::new();
    let reserved = space.try_reserve(7).unwrap();
    let identity = reserved.registration_id();
    let lease = reserved.publish(String::from("first"));
    assert_eq!(lease.registration_id(), identity);
    assert_eq!(lease.address(), &7);
    assert_eq!(space.len(), 1);
    let snapshot = space.resolve(&7).unwrap();
    lease.release();
    let fresh = space
        .try_reserve(7)
        .unwrap()
        .publish(String::from("second"));
    assert_ne!(fresh.registration_id(), identity);
    assert_eq!(snapshot.as_str(), "first");
    assert_eq!(space.resolve(&7).unwrap().as_str(), "second");
    drop(fresh);
    assert!(space.is_empty());
}

#[test]
fn publish_and_drop_survive_growth_and_shrink() {
    let space = AddressSpace::new();
    let reserved = space.try_reserve(0).unwrap();
    let abandoned = space.try_reserve(1).unwrap();
    let size = if cfg!(miri) { 32 } else { 4096 };
    let others: Vec<_> = (2..size)
        .map(|key| space.claim(key, key).unwrap())
        .collect();
    drop(others);
    space.shrink_to_fit();
    assert_eq!(space.len(), 2);
    drop(abandoned);
    let lease = reserved.publish(99);
    assert_eq!(space.resolve(&0).as_deref(), Some(&99));
    drop(lease);
    space.shrink_to_fit();
    assert!(space.is_empty());
}

#[test]
fn publish_does_not_clone_the_address_or_endpoint() {
    #[derive(Debug)]
    struct Key(Arc<AtomicUsize>);
    impl Clone for Key {
        fn clone(&self) -> Self {
            self.0.fetch_add(1, Ordering::SeqCst);
            Self(self.0.clone())
        }
    }
    impl PartialEq for Key {
        fn eq(&self, _: &Self) -> bool {
            true
        }
    }
    impl Eq for Key {}
    impl std::hash::Hash for Key {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            state.write_u8(1);
        }
    }
    struct NonClone;
    let clones = Arc::new(AtomicUsize::new(0));
    let space = AddressSpace::<Key, NonClone>::new();
    let reserved = space.try_reserve(Key(clones.clone())).unwrap();
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    let lease = reserved.publish(NonClone);
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    drop(lease);
    assert!(space.is_empty());
}

#[test]
fn duplicate_returns_original_address_allocation() {
    let space = AddressSpace::<_, ()>::new();
    let _reserved = space.try_reserve(String::from("worker")).unwrap();
    let duplicate = String::from("worker");
    let pointer = duplicate.as_ptr();
    let Err(ClaimError::AddressInUse(returned)) = space.try_reserve(duplicate) else {
        panic!("duplicate must fail");
    };
    assert_eq!(returned.as_ptr(), pointer);
}

#[test]
fn reservation_can_migrate_threads_and_outlive_space_handles() {
    let space = AddressSpace::<_, String>::new();
    let reserved = space.try_reserve(1).unwrap();
    let peer = space.clone();
    drop(space);
    let lease = std::thread::spawn(move || reserved.publish(String::from("ready")))
        .join()
        .unwrap();
    assert_eq!(peer.resolve(&1).unwrap().as_str(), "ready");
    drop(peer);
    drop(lease);
}

#[test]
fn publication_preserves_reentrant_endpoint_clone_and_drop() {
    struct Endpoint {
        space: AddressSpace<u64, Endpoint>,
        drops: Arc<AtomicUsize>,
    }
    impl Clone for Endpoint {
        fn clone(&self) -> Self {
            let scratch = self.space.try_reserve(2).unwrap();
            drop(scratch);
            Self {
                space: self.space.clone(),
                drops: self.drops.clone(),
            }
        }
    }
    impl Drop for Endpoint {
        fn drop(&mut self) {
            let scratch = self.space.try_reserve(2).unwrap();
            drop(scratch);
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    let space = AddressSpace::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let lease = space.try_reserve(1).unwrap().publish(Endpoint {
        space: space.clone(),
        drops: drops.clone(),
    });
    let snapshot = space.resolve(&1).unwrap();
    drop(lease);
    assert!(space.is_empty());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(snapshot);
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}

#[test]
fn reservation_key_drop_can_reenter_the_space() {
    struct Key {
        value: u64,
        on_drop: Arc<dyn Fn() + Send + Sync>,
    }
    impl Clone for Key {
        fn clone(&self) -> Self {
            Self {
                value: self.value,
                on_drop: self.on_drop.clone(),
            }
        }
    }
    impl PartialEq for Key {
        fn eq(&self, other: &Self) -> bool {
            self.value == other.value
        }
    }
    impl Eq for Key {}
    impl std::hash::Hash for Key {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            state.write_u64(self.value);
        }
    }
    impl Drop for Key {
        fn drop(&mut self) {
            (self.on_drop)();
        }
    }
    let space = AddressSpace::<Key, ()>::new();
    let peer = space.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let key = Key {
        value: 1,
        on_drop: Arc::new(move || {
            assert!(peer.is_empty());
            count.fetch_add(1, Ordering::SeqCst);
        }),
    };
    let reserved = space.try_reserve(key).ok().unwrap();
    drop(reserved);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn unwinding_endpoint_construction_releases_reservation() {
    let space = AddressSpace::<_, String>::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _reserved = space.try_reserve(1).unwrap();
        panic!("endpoint construction failed");
    }));
    assert!(result.is_err());
    assert!(space.is_empty());
    let _replacement = space.try_reserve(1).unwrap();
}
