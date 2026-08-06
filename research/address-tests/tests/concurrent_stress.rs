//! Deterministic adversarial concurrency stress.
//!
//! Every test uses fixed thread counts, fixed operation counts, and a
//! `Barrier` so all threads arrive at the contended section together —
//! real overlap, not sequential-then-check. No wall-clock randomness: the
//! workload is identical on every run.
//!
//! Excluded from Miri (thread-count × operation-count is tuned for native
//! execution); `tests/miri_ownership.rs` carries the Miri workload.
#![cfg(not(miri))]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use bombay_address::AddressSpace;

/// 8 threads fight over 64 shared addresses. An ownership counter per
/// address must never exceed 1: a successful claimant increments before
/// releasing, so any second live owner of the same address is observed.
#[test]
fn contended_claims_admit_at_most_one_owner() {
    const THREADS: usize = 8;
    const ROUNDS: usize = 2_000;
    const ADDRESSES: usize = 64;

    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let owners: Arc<Vec<AtomicUsize>> =
        Arc::new((0..ADDRESSES).map(|_| AtomicUsize::new(0)).collect());
    let barrier = Arc::new(Barrier::new(THREADS));

    let handles: Vec<_> = (0..THREADS)
        .map(|thread| {
            let space = Arc::clone(&space);
            let owners = Arc::clone(&owners);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for round in 0..ROUNDS {
                    // All threads walk the same address sequence, slightly
                    // offset, maximizing collisions on the same key.
                    let address = ((round + thread) % ADDRESSES) as u64;
                    if let Ok(lease) = space.claim(address, (thread * ROUNDS + round) as u64) {
                        let previous = owners[address as usize].fetch_add(1, Ordering::SeqCst);
                        assert_eq!(
                            previous, 0,
                            "address {address} has two live owners simultaneously"
                        );
                        // Hold the lease across several resolve attempts to
                        // widen the race window.
                        for _ in 0..4 {
                            let _ = space.resolve(&address);
                        }
                        owners[address as usize].fetch_sub(1, Ordering::SeqCst);
                        lease.release();
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    for (address, owner) in owners.iter().enumerate() {
        assert_eq!(owner.load(Ordering::SeqCst), 0, "address {address} leaked");
    }
    assert!(space.is_empty(), "all leases released; space must be empty");
}

/// Writers churn disjoint address ranges with endpoint values tagged by
/// their range. A resolve that returns an endpoint tagged for a different
/// range proves a cross-key mixup (wrong-bucket collision, torn key, or
/// stale entry resurfacing).
#[test]
fn resolve_never_returns_a_foreign_endpoint_during_churn() {
    const WRITERS: u64 = 4;
    const READERS: usize = 4;
    const RANGE: u64 = 256;
    const CHURN: u64 = 1_000;
    const TAG: u64 = 1 << 32;

    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let barrier = Arc::new(Barrier::new((WRITERS as usize) + READERS));
    let stop = Arc::new(AtomicU64::new(0));

    let writers: Vec<_> = (0..WRITERS)
        .map(|writer| {
            let space = Arc::clone(&space);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for round in 0..CHURN {
                    let address = writer * RANGE + (round % RANGE);
                    let endpoint = writer * TAG + round + 1;
                    let lease = space.claim(address, endpoint).unwrap();
                    lease.release();
                }
            })
        })
        .collect();
    let readers: Vec<_> = (0..READERS)
        .map(|reader| {
            let space = Arc::clone(&space);
            let barrier = Arc::clone(&barrier);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                barrier.wait();
                let mut probe = (reader as u64) * 61;
                while stop.load(Ordering::Relaxed) == 0 {
                    probe = probe.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    let address = probe % (WRITERS * RANGE);
                    if let Some(endpoint) = space.resolve(&address) {
                        let owner_range = endpoint / TAG;
                        let address_range = address / RANGE;
                        assert_eq!(
                            owner_range, address_range,
                            "resolve({address}) returned endpoint {endpoint} tagged \
                             for range {owner_range}"
                        );
                    }
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    stop.store(1, Ordering::Relaxed);
    for reader in readers {
        reader.join().unwrap();
    }
    assert!(space.is_empty());
}

/// Release racing resolve must never tear: readers of one hot address see
/// either absence or the exact endpoint of some generation, never a mix.
/// Writer claims strictly increasing endpoints; a reader that observes a
/// value larger than any claimed so far, or a foreign tag, fails.
#[test]
fn hot_address_release_resolve_race_stays_within_claimed_values() {
    const READERS: usize = 6;
    const GENERATIONS: u64 = 2_000;

    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let claimed = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(READERS + 1));
    let stop = Arc::new(AtomicU64::new(0));

    let writer = {
        let space = Arc::clone(&space);
        let claimed = Arc::clone(&claimed);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            for generation in 1..=GENERATIONS {
                // Advertise the generation BEFORE claiming: any resolve
                // that observes `generation` linearized after this store.
                claimed.store(generation, Ordering::Release);
                let lease = space.claim(0_u64, generation).unwrap();
                lease.release();
            }
        })
    };
    let readers: Vec<_> = (0..READERS)
        .map(|_| {
            let space = Arc::clone(&space);
            let claimed = Arc::clone(&claimed);
            let barrier = Arc::clone(&barrier);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                barrier.wait();
                while stop.load(Ordering::Relaxed) == 0 {
                    if let Some(endpoint) = space.resolve(&0) {
                        let high_water = claimed.load(Ordering::Acquire);
                        assert!(
                            endpoint >= 1 && endpoint <= high_water,
                            "resolve returned {endpoint}, outside claimed range 1..={high_water}"
                        );
                    }
                }
            })
        })
        .collect();
    writer.join().unwrap();
    stop.store(1, Ordering::Relaxed);
    for reader in readers {
        reader.join().unwrap();
    }
    assert!(space.is_empty());
}

/// Cloned spaces handed to different threads observe each other's
/// registrations (shared-state contract) under barrier-synchronized overlap.
#[test]
fn cloned_spaces_across_threads_share_registrations() {
    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let peer = AddressSpace::clone(&space);
    let gate = Arc::new(Barrier::new(2));
    let resolved = Arc::new(Barrier::new(2));

    let claimant = {
        let gate = Arc::clone(&gate);
        let resolved = Arc::clone(&resolved);
        std::thread::spawn(move || {
            gate.wait(); // start together
            let lease = peer.claim(42_u64, 1_000_u64).unwrap();
            gate.wait(); // claim landed; park until the observer resolved
            resolved.wait();
            lease.release();
            gate.wait(); // release landed
        })
    };
    gate.wait(); // start together
    gate.wait(); // claim landed; claimant is parked on `resolved`
    assert_eq!(space.resolve(&42), Some(1_000));
    resolved.wait(); // allow the release
    gate.wait(); // release landed
    assert_eq!(space.resolve(&42), None);
    claimant.join().unwrap();
    assert!(space.is_empty());
}

/// Concurrent lease handoff: claims on the producer thread are released
/// on a consumer thread over a channel while a third thread resolves.
/// Ownership counters must never exceed one per address, and the final
/// drain must be exact.
#[test]
fn cross_thread_lease_handoff_stays_exact() {
    const ROUNDS: u64 = 3_000;
    const ADDRESSES: u64 = 32;

    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let owners: Arc<Vec<AtomicUsize>> =
        Arc::new((0..ADDRESSES).map(|_| AtomicUsize::new(0)).collect());
    let (tx, rx) = std::sync::mpsc::channel::<bombay_address::Lease<u64, u64>>();
    let barrier = Arc::new(Barrier::new(3));

    let producer = {
        let space = Arc::clone(&space);
        let owners = Arc::clone(&owners);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            for round in 0..ROUNDS {
                let address = round % ADDRESSES;
                if let Ok(lease) = space.claim(address, round + 1) {
                    let previous = owners[address as usize].fetch_add(1, Ordering::SeqCst);
                    assert_eq!(previous, 0, "address {address} double-owned");
                    tx.send(lease).unwrap();
                }
            }
        })
    };
    let consumer = {
        let owners = Arc::clone(&owners);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            while let Ok(lease) = rx.recv() {
                let address = *lease.address();
                owners[address as usize].fetch_sub(1, Ordering::SeqCst);
                lease.release();
            }
        })
    };
    let resolver = {
        let space = Arc::clone(&space);
        std::thread::spawn(move || {
            barrier.wait();
            for round in 0..ROUNDS {
                let address = round % ADDRESSES;
                if let Some(endpoint) = space.resolve(&address) {
                    assert!((1..=ROUNDS).contains(&endpoint), "foreign endpoint {endpoint}");
                }
            }
        })
    };
    producer.join().unwrap();
    consumer.join().unwrap();
    resolver.join().unwrap();
    for (address, owner) in owners.iter().enumerate() {
        assert_eq!(owner.load(Ordering::SeqCst), 0, "address {address} leaked");
    }
    assert!(space.is_empty());
}

/// Contention with re-entrant endpoint drops: every endpoint's `Drop`
/// calls back into the space (`len`, taking the read guard). If any drop
/// ever ran under the write guard this would deadlock; the watchdog
/// converts a deadlock regression into a test failure instead of a hang.
#[test]
fn contended_reentrant_endpoint_drops_complete() {
    use std::sync::mpsc;
    use std::time::Duration;

    struct Reentrant {
        space: Arc<AddressSpace<u64, Reentrant>>,
    }
    impl Clone for Reentrant {
        fn clone(&self) -> Self {
            let _ = self.space.len();
            Self {
                space: Arc::clone(&self.space),
            }
        }
    }
    impl Drop for Reentrant {
        fn drop(&mut self) {
            let _ = self.space.len();
        }
    }

    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        const THREADS: usize = 6;
        const ROUNDS: u64 = 1_000;
        let space = Arc::new(AddressSpace::<u64, Reentrant>::new());
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS as u64)
            .map(|thread| {
                let space = Arc::clone(&space);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for round in 0..ROUNDS {
                        let address = (thread + round) % 16;
                        let endpoint = Reentrant {
                            space: Arc::clone(&space),
                        };
                        if let Ok(lease) = space.claim(address, endpoint) {
                            let _ = space.resolve(&address);
                            lease.release();
                        }
                        // Rejected endpoints also drop (re-entrantly).
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(space.is_empty());
        let _ = done_tx.send(());
    });
    assert!(
        done_rx.recv_timeout(Duration::from_secs(60)).is_ok(),
        "contended re-entrant endpoint drops deadlocked"
    );
}
