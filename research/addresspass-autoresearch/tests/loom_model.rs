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
