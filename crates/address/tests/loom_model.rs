//! Additional Loom models beyond the frozen `tests/loom.rs`.
//!
//! The frozen surface only exercises partial publication. These models
//! cover the other concurrency contracts: exactly-one-winner on duplicate
//! claims, release racing resolve (no torn reads), and replacement
//! (release + reclaim) never exposing a stale or torn endpoint.
//!
//! The frozen the Nix verification gate runs this suite together with `tests/loom.rs`.
//! To run it explicitly:
//! `LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" cargo test -p bombay-address
//! --test loom_model --release`
#![cfg(loom)]

use bombay_address::{AddressInUse, AddressSpace};
use loom::sync::Arc;
use loom::thread;

#[test]
fn concurrent_duplicate_claims_admit_exactly_one_owner() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let first_space = Arc::clone(&space);
        let second_space = Arc::clone(&space);

        let first = thread::spawn(move || first_space.claim(1_u64, 11_u64));
        let second = thread::spawn(move || second_space.claim(1_u64, 22_u64));
        let first_result = first.join().unwrap();
        let second_result = second.join().unwrap();

        // The write lock serializes the two claims: exactly one succeeds.
        let winner = match (&first_result, &second_result) {
            (Ok(_), Err(_)) => 11,
            (Err(_), Ok(_)) => 22,
            _ => panic!("exactly one concurrent claim must win"),
        };
        // The loser is rejected atomically with the contested address.
        for result in [&first_result, &second_result] {
            if let Err(AddressInUse(address)) = result {
                assert_eq!(*address, 1);
            }
        }
        // The winner's endpoint is live; the loser's never appears.
        assert_eq!(space.resolve(&1).as_deref().copied(), Some(winner));
    });
}

#[test]
fn release_racing_resolve_never_observes_torn_state() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let lease = space.claim(1_u64, 99_u64).unwrap();
        let reader_space = Arc::clone(&space);

        let writer = thread::spawn(move || drop(lease));
        let reader = thread::spawn(move || {
            if let Some(endpoint) = reader_space.resolve(&1) {
                assert_eq!(endpoint, 99);
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();
    });
}

#[test]
fn replacement_never_exposes_stale_or_torn_endpoint() {
    loom::model(|| {
        let space = Arc::new(AddressSpace::new());
        let lease = space.claim(1_u64, 10_u64).unwrap();
        let writer_space = Arc::clone(&space);
        let reader_space = Arc::clone(&space);

        let writer = thread::spawn(move || {
            drop(lease);
            let fresh = writer_space.claim(1_u64, 20_u64).expect("freed by drop");
            assert_eq!(writer_space.resolve(&1).as_deref().copied(), Some(20));
            drop(fresh);
        });
        let reader = thread::spawn(move || match reader_space.resolve(&1).as_deref().copied() {
            Some(10) | Some(20) | None => {}
            Some(other) => panic!("torn or stale endpoint observed: {other}"),
        });

        writer.join().unwrap();
        reader.join().unwrap();
        assert_eq!(space.resolve(&1), None);
    });
}
