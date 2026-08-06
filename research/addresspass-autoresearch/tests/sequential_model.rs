//! Sequential model-based verification against an independent reference
//! model, plus exhaustive exploration of all small operation histories.
//!
//! The reference model (`ReferenceModel`) is a plain `BTreeMap` encoding the
//! documented semantics; it shares no code with `addresspass`.

use addresspass::{AddressInUse, AddressSpace, Lease};
use addresspass_autoresearch::ReferenceModel;

/// One operation in a history. `Release(u8)` releases the lease currently
/// live for that address, if any — with two addresses this alphabet is
/// complete and every length-`d` sequence over it is a valid history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Claim(u8),
    Release(u8),
    Resolve(u8),
}

/// A tracked lease pairs the SUT lease with the model's generation so
/// releases can be replayed exactly against the model.
struct Tracked {
    lease: Lease<u64, u64>,
    generation: u64,
}

/// Replay a full history against a fresh SUT and a fresh model, asserting
/// every observable result agrees: claim success/failure, the address
/// returned in `AddressInUse`, every resolve result, and the live count.
fn replay<const N: usize>(path: &[Op]) {
    let space = AddressSpace::<u64, u64>::new();
    let mut model = ReferenceModel::new();
    let mut leases: [Option<Tracked>; N] = [(); N].map(|_| None);
    for (step, op) in path.iter().enumerate() {
        match *op {
            Op::Claim(address) => {
                // Deterministic, unique, non-zero endpoint per position.
                let endpoint = (step as u64) + 1;
                let address = u64::from(address);
                match space.claim(address, endpoint) {
                    Ok(lease) => {
                        let generation = model
                            .claim(address, endpoint)
                            .unwrap_or_else(|| {
                                panic!("step {step}: SUT claimed an owned address")
                            });
                        leases[address as usize] = Some(Tracked { lease, generation });
                    }
                    Err(AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(
                            model.claim(address, endpoint).is_none(),
                            "step {step}: SUT refused a free address"
                        );
                    }
                }
            }
            Op::Release(address) => {
                if let Some(tracked) = leases[address as usize].take() {
                    model.release(u64::from(address), tracked.generation);
                    drop(tracked.lease);
                }
            }
            Op::Resolve(address) => {
                assert_eq!(
                    space.resolve(&u64::from(address)),
                    model.resolve(u64::from(address)),
                    "step {step}: resolve({address}) diverged from model"
                );
            }
        }
        assert_eq!(space.len(), model.len(), "step {step}: len diverged");
    }
    for (address, slot) in leases.iter_mut().enumerate() {
        if let Some(tracked) = slot.take() {
            model.release(address as u64, tracked.generation);
            drop(tracked.lease);
        }
    }
    assert!(space.is_empty());
}

/// Exhaustively enumerate every history over `N` addresses up to `depth`
/// operations long: 3·N symbols per position (claim/resolve/release ×
/// address), (3·N)^depth leaves, each replayed from a clean state.
fn explore<const N: usize>(path: &mut Vec<Op>, depth: usize, histories: &mut u64) {
    if depth == 0 {
        replay::<N>(path);
        *histories += 1;
        return;
    }
    let symbols = (3 * N) as u8;
    for symbol in 0..symbols {
        let address = symbol % N as u8;
        let op = match symbol / N as u8 {
            0 => Op::Claim(address),
            1 => Op::Resolve(address),
            _ => Op::Release(address),
        };
        path.push(op);
        explore::<N>(path, depth - 1, histories);
        path.pop();
    }
}

fn explore_all<const N: usize>(max_depth: u32) -> u64 {
    let mut path = Vec::new();
    let mut histories = 0_u64;
    for depth in 1..=max_depth {
        explore::<N>(&mut path, depth as usize, &mut histories);
    }
    let expected: u64 = (1..=max_depth).map(|d| (3 * N as u64).pow(d)).sum();
    assert_eq!(histories, expected, "exploration must visit every history");
    histories
}

#[test]
// 335,941 histories × fresh AddressSpace each: native-only. The Miri lane
// covers the same code paths at small scale in `miri_ownership.rs`.
#[cfg_attr(miri, ignore = "exhaustive exploration is a native-speed workload")]
fn exhaustive_two_address_histories_up_to_depth_7_match_model() {
    assert_eq!(explore_all::<2>(7), 335_922);
}

#[test]
// 3 addresses (two live owners at once — cross-registration interaction),
// 9 symbols per position, depths 1..=6: 597,861 histories. Native-only.
#[cfg_attr(miri, ignore = "exhaustive exploration is a native-speed workload")]
fn exhaustive_three_address_histories_up_to_depth_6_match_model() {
    assert_eq!(explore_all::<3>(6), 597_870);
}

#[test]
// 4 addresses, 12 symbols per position, depths 1..=5: 271,452 histories.
// Native-only.
#[cfg_attr(miri, ignore = "exhaustive exploration is a native-speed workload")]
fn exhaustive_four_address_histories_up_to_depth_5_match_model() {
    assert_eq!(explore_all::<4>(5), 271_452);
}

#[test]
// 5 addresses, 15 symbols per position, depths 1..=4: 54,240 histories.
// Native-only.
#[cfg_attr(miri, ignore = "exhaustive exploration is a native-speed workload")]
fn exhaustive_five_address_histories_up_to_depth_4_match_model() {
    assert_eq!(explore_all::<5>(4), 54_240);
}

#[test]
// 6 addresses, 18 symbols per position, depths 1..=3: 6,174 histories.
// Complements the deepest cells (2-addr d7) with a wider fan-out — the
// first exhaustive lane over more than five simultaneous registrations.
// Native-only.
#[cfg_attr(miri, ignore = "exhaustive exploration is a native-speed workload")]
fn exhaustive_six_address_histories_up_to_depth_3_match_model() {
    assert_eq!(explore_all::<6>(3), 6_174);
}

#[test]
fn stale_generation_is_unreachable_through_the_public_api() {
    // Documented invariant: releasing an old generation cannot remove a
    // newer one. Through the public API a stale lease cannot exist — a
    // lease's address cannot be re-registered while the lease lives — so
    // the generation gate is exercised here as far as the API allows:
    // replacement after release gets a fresh registration, and the
    // consumed old lease cannot interfere.
    let space = AddressSpace::new();
    let first = space.claim(1_u64, 10_u64).unwrap();
    first.release();
    let second = space.claim(1_u64, 20_u64).unwrap();
    assert_eq!(space.resolve(&1), Some(20));
    second.release();
    assert_eq!(space.resolve(&1), None);
    assert!(space.is_empty());
}

#[test]
fn release_then_reclaim_then_release_is_exact() {
    let space = AddressSpace::new();
    let lease = space.claim(1_u64, 10_u64).unwrap();
    lease.release();
    // Re-registration works immediately after an explicit release.
    let second = space.claim(1_u64, 20_u64).unwrap();
    assert_eq!(space.resolve(&1), Some(20));
    second.release();
    assert_eq!(space.len(), 0);
}

#[test]
fn resolve_returns_independent_snapshots() {
    let space = AddressSpace::new();
    let lease = space.claim(1_u64, String::from("live")).unwrap();
    let mut snapshot = space.resolve(&1).unwrap();
    snapshot.push_str("-mutated");
    assert_eq!(space.resolve(&1).as_deref(), Some("live"));
    drop(lease);
}

#[test]
fn failed_claim_drops_the_rejected_endpoint_outside_the_lock() {
    // The rejected endpoint's Drop re-enters the address space; if the drop
    // ran under the write guard this would self-deadlock (parking_lot
    // locks are not re-entrant).
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct ReenterOnDrop {
        space: Arc<AddressSpace<u64, ReenterOnDrop>>,
        probe: u64,
        drops: Arc<AtomicU64>,
    }
    impl Clone for ReenterOnDrop {
        fn clone(&self) -> Self {
            // Clone also re-enters (resolve runs endpoint Clone after the
            // read guard; claim runs key Clone before the write guard).
            // Uses `len` (no endpoint Clone) to stay non-recursive.
            let _ = self.space.len();
            Self {
                space: Arc::clone(&self.space),
                probe: self.probe,
                drops: Arc::clone(&self.drops),
            }
        }
    }
    impl Drop for ReenterOnDrop {
        fn drop(&mut self) {
            // Re-enters via `len` (read lock): detects a drop under the
            // write guard without recursively cloning endpoints.
            let _ = self.space.len();
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    let space = Arc::new(AddressSpace::new());
    let drops = Arc::new(AtomicU64::new(0));
    let owner = ReenterOnDrop {
        space: Arc::clone(&space),
        probe: 0,
        drops: Arc::clone(&drops),
    };
    let lease = space.claim(0_u64, owner).unwrap();
    let loser = ReenterOnDrop {
        space: Arc::clone(&space),
        probe: 0,
        drops: Arc::clone(&drops),
    };
    assert!(matches!(space.claim(0_u64, loser), Err(AddressInUse(0))));
    // Loser was dropped exactly once, outside the lock (no deadlock, and
    // the drop ran: the counter moved).
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    // The live endpoint is still the winner; resolve runs Clone which
    // re-enters without deadlock. The returned snapshot is a temporary and
    // drops at the end of the statement (+1).
    assert!(space.resolve(&0).is_some());
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    drop(lease);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
    assert!(space.is_empty());
}

#[test]
fn released_endpoint_drop_reenters_safely_and_observes_absence() {
    // On release, the removed endpoint must be dropped after the write
    // guard is released; its re-entrant resolve must observe the address
    // already gone (removal is linearized before the drop runs).
    use parking_lot::Mutex;
    use std::sync::Arc;

    struct Probe {
        space: Arc<AddressSpace<u64, Probe>>,
        address: u64,
        observed: Arc<Mutex<Option<bool>>>,
    }
    impl Clone for Probe {
        fn clone(&self) -> Self {
            Self {
                space: Arc::clone(&self.space),
                address: self.address,
                observed: Arc::clone(&self.observed),
            }
        }
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            let present = self.space.resolve(&self.address).is_some();
            *self.observed.lock() = Some(present);
        }
    }

    let space = Arc::new(AddressSpace::new());
    let observed = Arc::new(Mutex::new(None));
    let probe = Probe {
        space: Arc::clone(&space),
        address: 7,
        observed: Arc::clone(&observed),
    };
    let lease = space.claim(7_u64, probe).unwrap();
    lease.release();
    assert_eq!(*observed.lock(), Some(false));
}

#[test]
fn hash_colliding_addresses_remain_fully_correct() {
    // Keys that always hash to the same bucket stress the collision path:
    // collision exclusion, resolution, and release must be unaffected.
    use std::hash::{Hash, Hasher};

    #[derive(Clone, PartialEq, Eq, Debug)]
    struct Colliding(u64);
    impl Hash for Colliding {
        fn hash<H: Hasher>(&self, state: &mut H) {
            // Constant hash: every key lands in one bucket.
            state.write_u64(0);
        }
    }

    let space = AddressSpace::new();
    let mut model = ReferenceModel::new();
    let mut leases = Vec::new();
    // 1,000 colliding keys natively; Miri interprets the same paths at 64.
    let population: u64 = if cfg!(miri) { 64 } else { 1_000 };
    for i in 0..population {
        let lease = space.claim(Colliding(i), i).unwrap();
        assert_eq!(model.claim(i, i), Some(i + 1));
        leases.push(lease);
    }
    assert_eq!(space.len(), population as usize);
    for i in 0..population {
        assert_eq!(space.resolve(&Colliding(i)), Some(i));
    }
    // Release every third lease, verify survivors, then drain the rest.
    let mut pending: Vec<(usize, Lease<Colliding, u64>)> =
        leases.into_iter().enumerate().collect();
    for (i, lease) in pending.extract_if(.., |(i, _)| *i % 3 == 0) {
        let i = i as u64;
        lease.release();
        model.release(i, i + 1);
        assert_eq!(space.resolve(&Colliding(i)), None);
    }
    for (i, _) in &pending {
        let i = *i as u64;
        assert_eq!(space.resolve(&Colliding(i)), Some(i));
    }
    for (i, lease) in pending {
        let i = i as u64;
        lease.release();
        model.release(i, i + 1);
    }
    assert_eq!(space.len(), model.len());
    assert!(space.is_empty());
}
