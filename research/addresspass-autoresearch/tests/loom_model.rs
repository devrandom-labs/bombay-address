//! Bounded Loom models for the concurrency contracts not covered by the
//! production crate's own loom tests: three-way claim contention and
//! multi-reader resolution during a release/reclaim cycle.
//!
//! Run explicitly:
//! `LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" cargo test \
//!   --manifest-path research/addresspass-autoresearch/Cargo.toml \
//!   --test loom_model --release`
//!
//! The preemption bound is reported honestly in RESEARCH-REPORT.md; these
//! models are exhaustive interleaving checks within that bound.
#![cfg(loom)]

use addresspass::AddressSpace;
use loom::sync::Arc;
use loom::thread;

/// Three concurrent claimants on one address: exactly one wins, the two
/// losers observe `AddressInUse`, and the winner's endpoint is the only one
/// ever visible.
#[test]
fn three_claimants_exactly_one_owner() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let claimants: Vec<_> = (1..=3_u64)
            .map(|endpoint| {
                let space = Arc::clone(&space);
                thread::spawn(move || space.claim(0_u64, endpoint))
            })
            .collect();
        let results: Vec<_> = claimants.into_iter().map(|c| c.join().unwrap()).collect();
        let winners: Vec<_> = results.iter().filter(|r| r.is_ok()).collect();
        assert_eq!(winners.len(), 1, "exactly one claimant may win");
        let winner = results
            .iter()
            .enumerate()
            .find(|(_, r)| r.is_ok())
            .map(|(i, _)| (i as u64) + 1)
            .unwrap();
        assert_eq!(space.resolve(&0), Some(winner));
        // The winning lease is dropped at scope end; the space drains.
        drop(results);
        assert_eq!(space.resolve(&0), None);
    });
}

/// A writer that releases and immediately reclaims one address while two
/// readers resolve in a loop: readers may observe the old endpoint, the new
/// endpoint, or absence — never anything else, and never a value after the
/// writer's final release.
#[test]
fn two_readers_observe_only_current_or_absent_during_reclaim() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let first = space.claim(0_u64, 10_u64).unwrap();

        let writer = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                drop(first);
                let second = space.claim(0_u64, 20_u64).expect("released above");
                drop(second);
            })
        };
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let space = Arc::clone(&space);
                thread::spawn(move || {
                    if let Some(endpoint) = space.resolve(&0) {
                        assert!(
                            endpoint == 10 || endpoint == 20,
                            "torn or foreign endpoint observed: {endpoint}"
                        );
                    }
                })
            })
            .collect();
        writer.join().unwrap();
        for reader in readers {
            reader.join().unwrap();
        }
        assert_eq!(space.resolve(&0), None);
    });
}

/// A resolve that overlaps the *final* release of an address may return the
/// endpoint (snapshot taken before the release linearized) or `None`; both
/// are linearizable. The endpoint Arc keeps the snapshot alive regardless.
#[test]
fn resolve_overlapping_final_release_returns_snapshot_or_absence() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let lease = space.claim(0_u64, 77_u64).unwrap();

        let releaser = thread::spawn(move || drop(lease));
        let resolver = {
            let space = Arc::clone(&space);
            thread::spawn(move || match space.resolve(&0) {
                Some(77) | None => {}
                Some(other) => panic!("impossible endpoint {other}"),
            })
        };
        releaser.join().unwrap();
        resolver.join().unwrap();
        assert_eq!(space.resolve(&0), None);
        assert!(space.is_empty());
    });
}

/// FINDING-004: the loom build's poison recovery is unreachable.
///
/// Production's `cfg(loom)` path uses `loom::sync::RwLock` with
/// `unwrap_or_else(recover)` to survive a poisoned lock. But loom 0.7's
/// `RwLock::read()`/`write()` NEVER return `Err(PoisonError)`: on a
/// poisoned lock they panic in-band (`"loom::RwLock state corrupt"`)
/// before handing out any result, so `recover` is dead code and ANY panic
/// while holding the write guard makes every subsequent operation on the
/// space panic too — in the loom build, one bad caller `Hash` permanently
/// kills the address space. (Native builds use parking_lot, which never
/// poisons, so `recover` is dead code there as well.)
///
/// The active assertion expresses the correct behavior: operations after
/// a caught poisoning panic recover and observe a consistent table.
#[test]
#[ignore = "FINDING-004: loom::RwLock panics in-band on poisoned state; production's recover() is unreachable and the space dies permanently after any mid-write panic"]
fn poisoned_write_lock_recovers_and_stays_consistent() {
    use std::hash::{Hash, Hasher};
    use std::panic::{AssertUnwindSafe, catch_unwind};

    #[derive(Clone, Debug)]
    struct MaybePanic {
        id: u64,
        panic: bool,
    }
    impl PartialEq for MaybePanic {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }
    impl Eq for MaybePanic {}
    impl Hash for MaybePanic {
        fn hash<H: Hasher>(&self, state: &mut H) {
            if self.panic {
                panic!("injected hash panic under the write guard");
            }
            state.write_u64(self.id);
        }
    }

    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let crasher = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    let _ = space.claim(MaybePanic { id: 1, panic: true }, 10_u64);
                }));
                assert!(result.is_err(), "injected panic must propagate");
            })
        };
        crasher.join().unwrap();

        // The write lock was poisoned mid-claim; recovery must yield a
        // working, consistent space with no ghost registration.
        assert!(space.is_empty());
        let good = MaybePanic {
            id: 1,
            panic: false,
        };
        let lease = space.claim(good.clone(), 20_u64).unwrap();
        assert_eq!(space.resolve(&good), Some(20));
        lease.release();
        assert!(space.is_empty());
    });
}

/// A release racing a fresh claim of the same address: the claim may fail
/// (linearized before the release) or succeed (after it) — but afterwards
/// the space must contain exactly the new registration or be empty, never
/// the old endpoint, never both.
#[test]
fn release_racing_claim_leaves_exact_new_owner_or_empty() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let old = space.claim(0_u64, 10_u64).unwrap();

        let releaser = thread::spawn(move || drop(old));
        let claimant = {
            let space = Arc::clone(&space);
            thread::spawn(move || space.claim(0_u64, 20_u64))
        };
        releaser.join().unwrap();
        let outcome = claimant.join().unwrap();

        match outcome {
            Ok(lease) => {
                assert_eq!(space.resolve(&0), Some(20));
                drop(lease);
                assert!(space.is_empty());
            }
            Err(_) => {
                assert_eq!(space.resolve(&0), None);
                assert!(space.is_empty());
            }
        }
    });
}

/// Two addresses, three threads: claimant on A, releaser+reclaimer on B,
/// reader alternating between both. Every read of either address must be
/// absent or the exact live endpoint of that address — no cross-address
/// bleed under interleaving.
#[test]
fn two_address_three_thread_interleaving_has_no_cross_bleed() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let lease_a = space.claim(1_u64, 100_u64).unwrap();
        let lease_b = space.claim(2_u64, 200_u64).unwrap();

        let holder_a = thread::spawn(move || {
            assert_eq!(*lease_a.address(), 1);
            drop(lease_a);
        });
        let swapper_b = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                drop(lease_b);
                let fresh = space.claim(2_u64, 300_u64).expect("freed above");
                drop(fresh);
            })
        };
        let reader = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                if let Some(v) = space.resolve(&1) {
                    assert_eq!(v, 100, "address 1 bled: {v}");
                }
                if let Some(v) = space.resolve(&2) {
                    assert!(v == 200 || v == 300, "address 2 bled: {v}");
                }
            })
        };
        holder_a.join().unwrap();
        swapper_b.join().unwrap();
        reader.join().unwrap();
        assert!(space.is_empty());
    });
}

/// Four claimants over two addresses: each address admits exactly one
/// owner, and afterwards both registrations drain exactly.
#[test]
fn four_claimants_two_addresses_exact_ownership() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let claimants: Vec<_> = (0..4_u64)
            .map(|i| {
                let space = Arc::clone(&space);
                let address = i % 2;
                let endpoint = (i + 1) * 10;
                thread::spawn(move || space.claim(address, endpoint))
            })
            .collect();
        let mut leases = Vec::new();
        for claimant in claimants {
            if let Ok(lease) = claimant.join().unwrap() {
                leases.push(lease);
            }
        }
        // Exactly one winner per address.
        assert_eq!(leases.len(), 2);
        let winner0 = space.resolve(&0).unwrap();
        let winner1 = space.resolve(&1).unwrap();
        assert!(winner0 == 10 || winner0 == 30, "address 0 winner {winner0}");
        assert!(winner1 == 20 || winner1 == 40, "address 1 winner {winner1}");
        for lease in leases {
            drop(lease);
        }
        assert!(space.is_empty());
    });
}

/// Nested release under interleaving: the endpoint at address 1 owns the
/// lease for address 2. Releasing address 1 cascades through the
/// endpoint's drop; a concurrent resolver of address 2 must observe
/// either its endpoint or absence — never a torn state — and the final
/// drain must be exact.
#[test]
fn nested_release_racing_resolve_stays_consistent() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());

        struct Holds {
            value: u64,
            #[expect(dead_code, reason = "exercised through drop, never read")]
            inner: Option<Box<addresspass::Lease<u64, Holds>>>,
        }
        impl Clone for Holds {
            fn clone(&self) -> Self {
                Holds {
                    value: self.value,
                    inner: None, // clones never carry a lease (one-shot)
                }
            }
        }

        let inner = space
            .claim(2_u64, Holds {
                value: 200,
                inner: None,
            })
            .unwrap();
        let outer = space
            .claim(1_u64, Holds {
                value: 100,
                inner: Some(Box::new(inner)),
            })
            .unwrap();

        let releaser = thread::spawn(move || drop(outer));
        let resolver = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                if let Some(v) = space.resolve(&2) {
                    assert_eq!(v.value, 200, "torn nested release observed");
                }
            })
        };
        releaser.join().unwrap();
        resolver.join().unwrap();
        // The cascade may or may not have completed for the resolver's
        // schedule, but after the releaser joined, both are gone.
        assert!(space.resolve(&1).is_none());
        assert!(space.resolve(&2).is_none());
        assert!(space.is_empty());
    });
}

/// Two endpoints whose `Drop` both CLAIM the same spawn address, racing
/// each other and a resolver: exactly one spawn claim wins (exclusive
/// ownership of 3), the resolver observes only the winner's value or
/// absence — never a torn or foreign value — and the parked winner
/// drains exactly.
#[test]
fn reentrant_spawns_race_exactly_one_wins() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let bag = loom::sync::Arc::new(loom::sync::Mutex::new(Vec::new()));

        struct Spawner {
            value: u64,
            space: loom::sync::Arc<AddressSpace<u64, Box<Spawner>>>,
            bag: loom::sync::Arc<loom::sync::Mutex<Vec<addresspass::Lease<u64, Box<Spawner>>>>>,
            spawn: Option<u64>,
        }
        impl Clone for Spawner {
            fn clone(&self) -> Self {
                Spawner {
                    value: self.value,
                    space: loom::sync::Arc::clone(&self.space),
                    bag: loom::sync::Arc::clone(&self.bag),
                    spawn: None, // snapshots and spawned endpoints never spawn again
                }
            }
        }
        impl Drop for Spawner {
            fn drop(&mut self) {
                if let Some(spawn) = self.spawn {
                    let spawned = Spawner {
                        value: self.value,
                        space: loom::sync::Arc::clone(&self.space),
                        bag: loom::sync::Arc::clone(&self.bag),
                        spawn: None,
                    };
                    if let Ok(lease) = self.space.claim(spawn, Box::new(spawned)) {
                        self.bag.lock().unwrap().push(lease);
                    }
                }
            }
        }

        let spawner = |value| Box::new(Spawner {
            value,
            space: loom::sync::Arc::clone(&space),
            bag: loom::sync::Arc::clone(&bag),
            spawn: Some(3),
        });
        let first = space.claim(1_u64, spawner(101)).unwrap();
        let second = space.claim(2_u64, spawner(102)).unwrap();

        let releaser1 = thread::spawn(move || drop(first));
        let releaser2 = thread::spawn(move || drop(second));
        let resolver = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                if let Some(v) = space.resolve(&3) {
                    // Only a winner's value — 101 or 102 — never torn.
                    assert!(v.value == 101 || v.value == 102, "foreign spawn value");
                }
            })
        };
        releaser1.join().unwrap();
        releaser2.join().unwrap();
        resolver.join().unwrap();

        // Exactly one spawn claim won: 3 is live with one of the two
        // values, and the bag holds exactly that one lease.
        let parked = bag.lock().unwrap().drain(..).collect::<Vec<_>>();
        assert_eq!(parked.len(), 1, "exactly one spawn must win");
        for lease in parked {
            drop(lease);
        }
        assert!(space.resolve(&3).is_none());
        assert!(space.is_empty());
    });
}

/// Two INDEPENDENT held-lease trees (release-graph shape: endpoint 1
/// holds the lease of 3; endpoint 2 holds the lease of 4) released
/// concurrently while resolvers watch the inner addresses 3 and 4:
/// each resolver observes only its own tree's value or absence — no
/// cross-tree bleed — and both trees drain exactly.
#[test]
fn concurrent_multi_tree_cascades_do_not_bleed() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());

        struct Holds {
            value: u64,
            #[expect(dead_code, reason = "exercised through drop, never read")]
            inner: Option<Box<addresspass::Lease<u64, Holds>>>,
        }
        impl Clone for Holds {
            fn clone(&self) -> Self {
                Holds {
                    value: self.value,
                    inner: None, // clones never carry a lease (one-shot)
                }
            }
        }

        let leaf3 = space
            .claim(3_u64, Holds { value: 300, inner: None })
            .unwrap();
        let tree1 = space
            .claim(1_u64, Holds {
                value: 100,
                inner: Some(Box::new(leaf3)),
            })
            .unwrap();
        let leaf4 = space
            .claim(4_u64, Holds { value: 400, inner: None })
            .unwrap();
        let tree2 = space
            .claim(2_u64, Holds {
                value: 200,
                inner: Some(Box::new(leaf4)),
            })
            .unwrap();

        let releaser1 = thread::spawn(move || drop(tree1));
        let releaser2 = thread::spawn(move || drop(tree2));
        let resolver3 = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                if let Some(v) = space.resolve(&3) {
                    assert_eq!(v.value, 300, "tree 1 resolver saw a foreign value");
                }
            })
        };
        let resolver4 = {
            let space = Arc::clone(&space);
            thread::spawn(move || {
                if let Some(v) = space.resolve(&4) {
                    assert_eq!(v.value, 400, "tree 2 resolver saw a foreign value");
                }
            })
        };
        releaser1.join().unwrap();
        releaser2.join().unwrap();
        resolver3.join().unwrap();
        resolver4.join().unwrap();
        // Both trees fully cascaded.
        assert!(space.is_empty());
    });
}
