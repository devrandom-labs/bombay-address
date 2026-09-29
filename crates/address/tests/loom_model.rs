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

#[test]
fn concurrent_reservations_admit_exactly_one_owner() {
    loom::model(|| {
        let space = AddressSpace::<u64, u64>::new();
        let peer = space.clone();
        let first = thread::spawn(move || peer.try_reserve(1));
        let second = space.try_reserve(1);
        let first = first.join().unwrap();
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert!(space.resolve(&1).is_none());
        assert_eq!(space.len(), 1);
        drop((first, second));
        assert!(space.is_empty());
    });
}

#[test]
fn reservation_racing_claim_has_one_owner() {
    loom::model(|| {
        let space = AddressSpace::new();
        let peer = space.clone();
        let reserved = thread::spawn(move || peer.try_reserve(1));
        let claimed = space.try_claim(1, 99);
        let reserved = reserved.join().unwrap();
        assert_eq!(
            usize::from(reserved.is_ok()) + usize::from(claimed.is_ok()),
            1
        );
        assert_eq!(
            space.resolve(&1).as_deref().copied(),
            claimed.as_ref().ok().map(|_| 99)
        );
    });
}

#[test]
fn publish_racing_resolve_and_claim_is_atomic() {
    loom::model(|| {
        let space = AddressSpace::new();
        let reserved = space.try_reserve(1).unwrap();
        let identity = reserved.registration_id();
        let peer = space.clone();
        let publisher = thread::spawn(move || reserved.publish((17, 29)));
        let contender = thread::spawn(move || {
            assert!(peer.try_claim(1, (0, 0)).is_err());
            assert!(peer.try_reserve(1).is_err());
        });
        if let Some(snapshot) = space.resolve(&1) {
            assert_eq!(*snapshot, (17, 29));
        }
        let lease = publisher.join().unwrap();
        contender.join().unwrap();
        assert_eq!(lease.registration_id(), identity);
        assert_eq!(space.resolve(&1).as_deref(), Some(&(17, 29)));
        drop(lease);
    });
}

#[test]
fn reservation_drop_racing_reacquisition_and_resolve_is_generation_safe() {
    loom::model(|| {
        let space = AddressSpace::new();
        let reserved = space.try_reserve(1).unwrap();
        let old_id = reserved.registration_id();
        let peer = space.clone();
        let releaser = thread::spawn(move || drop(reserved));
        let acquirer = thread::spawn(move || peer.try_reserve(1).map(|r| r.publish(99)));
        if let Some(snapshot) = space.resolve(&1) {
            assert_eq!(*snapshot, 99);
        }
        releaser.join().unwrap();
        let replacement = acquirer.join().unwrap();
        if let Ok(lease) = replacement {
            assert_ne!(lease.registration_id(), old_id);
            assert_eq!(space.resolve(&1).as_deref(), Some(&99));
            drop(lease);
        }
        assert!(space.is_empty());
    });
}

#[test]
fn publish_release_and_replacement_preserve_snapshots() {
    loom::model(|| {
        let space = AddressSpace::new();
        let reserved = space.try_reserve(1).unwrap();
        let peer = space.clone();
        let writer = thread::spawn(move || {
            reserved.publish((1, 1)).release();
            peer.try_reserve(1).unwrap().publish((2, 2))
        });
        let snapshot = space.resolve(&1);
        if let Some(value) = &snapshot {
            assert!(**value == (1, 1) || **value == (2, 2));
        }
        let lease = writer.join().unwrap();
        assert_eq!(space.resolve(&1).as_deref(), Some(&(2, 2)));
        drop(lease);
        if let Some(value) = snapshot {
            assert!(*value == (1, 1) || *value == (2, 2));
        }
    });
}
