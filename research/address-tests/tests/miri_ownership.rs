//! Miri workload: ownership, drop, and snapshot-lifetime paths exercised
//! with small, bounded loops so the interpreter stays tractable.
//!
//! Run:
//! `nix develop .#miri --command cargo miri test \
//!   --manifest-path research/address-tests/Cargo.toml`
//!
//! These tests are also ordinary native tests (no `cfg(miri)` gate), so the
//! standard `cargo test` gate runs them too.

use bombay_address::AddressSpace;
use address_tests::ReferenceModel;

/// A small model-checked history with real ownership transfer: endpoints
/// are heap-allocated strings whose drops Miri tracks for use-after-free
/// and leaks.
#[test]
fn miri_model_checked_history_with_heap_endpoints() {
    let space = AddressSpace::new();
    let mut model = ReferenceModel::new();
    let mut leases = Vec::new();
    // Deterministic hand-written history covering claim/resolve/release
    // overlap on colliding-adjacent addresses.
    let script: [(u8, u8); 24] = [
        (1, 0), (1, 1), (1, 0), (2, 0), (3, 0), (1, 2), (3, 1), (2, 1), (1, 1), (3, 2), (2, 0),
        (3, 0), (1, 3), (3, 3), (2, 2), (1, 0), (2, 3), (3, 0), (2, 1), (3, 1), (1, 2), (2, 2),
        (3, 2), (3, 3),
    ];
    // op: 1 = claim address n, 2 = resolve address n, 3 = release lease for
    // address n if live. Endpoints are heap strings to exercise Miri's
    // ownership tracking.
    let mut generations: std::collections::BTreeMap<u64, (u64, usize)> =
        std::collections::BTreeMap::new();
    let mut endpoint = 0_u64;
    for (step, (op, n)) in script.iter().enumerate() {
        let address = u64::from(*n);
        match op {
            1 => {
                endpoint += 1;
                let value = format!("endpoint-{endpoint}");
                match space.claim(address, value.clone()) {
                    Ok(lease) => {
                        let generation = model.claim(address, endpoint).unwrap();
                        generations.insert(address, (generation, leases.len()));
                        leases.push(Some(lease));
                    }
                    Err(_) => {
                        assert!(model.claim(address, endpoint).is_none(), "step {step}");
                    }
                }
            }
            2 => {
                let expected = model.resolve(address).map(|e| format!("endpoint-{e}"));
                assert_eq!(
                    space.resolve(&address).as_deref().cloned(),
                    expected,
                    "step {step}"
                );
            }
            3 => {
                if let Some((generation, index)) = generations.remove(&address) {
                    let lease = leases[index].take().unwrap();
                    model.release(address, generation);
                    drop(lease);
                }
            }
            _ => unreachable!(),
        }
        assert_eq!(space.len(), model.len(), "step {step}");
    }
    for lease in leases.into_iter().flatten() {
        drop(lease);
    }
    assert!(space.is_empty());
}

/// Resolve snapshots must remain valid after the registration they came
/// from is released (Arc-kept endpoint), and releasing must not invalidate
/// previously returned snapshots.
#[test]
fn miri_snapshot_outlives_release() {
    let space = AddressSpace::new();
    let lease = space.claim(0_u64, String::from("snapshot-data")).unwrap();
    let snapshot = space.resolve(&0).unwrap();
    lease.release();
    assert_eq!(snapshot.as_str(), "snapshot-data");
    assert_eq!(space.resolve(&0), None);
}

/// Concurrent claim/release with a handful of threads at tiny scale: Miri
/// checks the synchronization itself for data races.
#[test]
fn miri_tiny_threaded_churn() {
    use std::sync::{Arc, Barrier};

    let space = Arc::new(AddressSpace::<u64, u64>::new());
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..3_u64)
        .map(|thread| {
            let space = Arc::clone(&space);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for round in 0..10 {
                    let address = (thread + round) % 4;
                    if let Ok(lease) = space.claim(address, thread * 100 + round) {
                        let _ = space.resolve(&address);
                        lease.release();
                    }
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert!(space.is_empty());
}

/// An endpoint whose `Drop` CLAIMS a new registration in the same space
/// (reentrant spawn, parked in a bag so the spawned registration
/// persists): the drop-time claim runs outside the write guard, and the
/// spawned endpoint's ownership is Miri-tracked (heap box) for leaks and
/// use-after-free. Small and deterministic so the interpreter stays
/// tractable.
#[test]
fn miri_reentrant_spawn_claims_persistent_registration() {
    use parking_lot::Mutex;
    use std::sync::Arc;

    let space = Arc::new(AddressSpace::<u64, Box<Spawner>>::new());
    type Bag = Arc<Mutex<Vec<bombay_address::Lease<u64, Box<Spawner>>>>>;
    let bag: Bag = Arc::new(Mutex::new(Vec::new()));

    struct Spawner {
        value: u64,
        space: Arc<AddressSpace<u64, Box<Spawner>>>,
        bag: Bag,
        spawn: Option<u64>,
    }
    impl Clone for Spawner {
        fn clone(&self) -> Self {
            Spawner {
                value: self.value,
                space: Arc::clone(&self.space),
                bag: Arc::clone(&self.bag),
                spawn: None, // snapshots and spawned endpoints never spawn again
            }
        }
    }
    impl Drop for Spawner {
        fn drop(&mut self) {
            if let Some(spawn) = self.spawn {
                let spawned = Spawner {
                    value: self.value,
                    space: Arc::clone(&self.space),
                    bag: Arc::clone(&self.bag),
                    spawn: None,
                };
                if let Ok(lease) = self.space.claim(spawn, Box::new(spawned)) {
                    self.bag.lock().push(lease);
                }
            }
        }
    }

    let spawner = |value| {
        Box::new(Spawner {
            value,
            space: Arc::clone(&space),
            bag: Arc::clone(&bag),
            spawn: Some(2),
        })
    };
    let first = space.claim(0_u64, spawner(100)).unwrap();
    let second = space.claim(1_u64, spawner(200)).unwrap();
    assert_eq!(space.len(), 2);

    // Release both: each drop spawn-claims address 2; exactly one wins.
    first.release();
    second.release();
    assert_eq!(space.len(), 1, "exactly one spawn must win");
    assert!(space.resolve(&2).is_some());

    let parked = bag.lock().drain(..).collect::<Vec<_>>();
    assert_eq!(parked.len(), 1);
    for lease in parked {
        drop(lease);
    }
    assert!(space.is_empty());
}
