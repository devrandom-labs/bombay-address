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
