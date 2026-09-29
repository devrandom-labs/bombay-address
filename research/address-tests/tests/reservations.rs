use address_tests::reservations::fuzz_entry;

#[cfg(not(miri))]
use proptest::prelude::*;

#[cfg(not(miri))]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]
    #[test]
    fn reservation_histories_match_independent_model(data in prop::collection::vec(any::<u8>(), 0..256)) {
        fuzz_entry(&data);
    }
}

#[test]
fn reserve_publish_release_history_with_colliding_and_heap_keys() {
    fuzz_entry(&[0, 0, 3, 6, 2, 6, 5, 3, 6, 4, 0, 7, 2, 6, 4, 8, 10, 14, 12]);
}

#[test]
fn concurrent_reserve_claim_publish_release_stress() {
    use bombay_address::AddressSpace;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    let space = AddressSpace::new();
    let owners = Arc::new(std::array::from_fn::<_, 8, _>(|_| AtomicUsize::new(0)));
    let barrier = Arc::new(Barrier::new(4));
    let rounds = if cfg!(miri) { 4 } else { 20_000 };
    std::thread::scope(|scope| {
        for worker in 0..4 {
            let space = &space;
            let owners = &owners;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                for round in 0..rounds {
                    let address = (round + worker) % 8;
                    let value = (address, worker, round);
                    if round % 2 == 0 {
                        if let Ok(r) = space.try_reserve(address) {
                            assert_eq!(owners[address].fetch_add(1, Ordering::SeqCst), 0);
                            assert!(space.resolve(&address).is_none());
                            if round % 4 == 0 {
                                let lease = r.publish(value);
                                assert_eq!(space.resolve(&address).as_deref(), Some(&value));
                                assert_eq!(owners[address].fetch_sub(1, Ordering::SeqCst), 1);
                                lease.release();
                            } else {
                                assert_eq!(owners[address].fetch_sub(1, Ordering::SeqCst), 1);
                                drop(r);
                            }
                        }
                    } else if let Ok(lease) = space.try_claim(address, value) {
                        assert_eq!(owners[address].fetch_add(1, Ordering::SeqCst), 0);
                        assert_eq!(space.resolve(&address).as_deref(), Some(&value));
                        assert_eq!(owners[address].fetch_sub(1, Ordering::SeqCst), 1);
                        drop(lease);
                    }
                    if let Some(snapshot) = space.resolve(&address) {
                        assert_eq!(snapshot.0, address);
                        assert_eq!((snapshot.2 + snapshot.1) % 8, address);
                    }
                }
            });
        }
    });
    assert!(space.is_empty());
}
