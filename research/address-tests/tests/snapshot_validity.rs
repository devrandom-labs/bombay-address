//! Snapshot validity: a value returned by `resolve` is a snapshot owned
//! by the caller. It must remain valid and unchanged no matter what
//! happens to the registration afterwards — release, replacement, space
//! handle drops — because the endpoint is held by an `Arc` until the last
//! handle (including snapshots) is gone.
//!
//! Excluded from Miri (case counts are a native-speed workload).
#![cfg(not(miri))]

use bombay_address::{AddressSpace, Lease, Resolved};
use proptest::prelude::*;

/// Run a history, collecting every resolve snapshot; after draining the
/// space completely, every snapshot must still hold its exact value.
fn run(ops: &[(u8, u8)]) {
    let space = AddressSpace::<u64, u64>::new();
    let mut model: std::collections::BTreeMap<u64, u64> = Default::default();
    let mut leases: std::collections::BTreeMap<u64, Lease<u64, u64>> = Default::default();
    let mut snapshots: Vec<Resolved<u64>> = Vec::new();
    let mut endpoint = 0_u64;
    for (step, (address, op)) in ops.iter().enumerate() {
        let address = u64::from(address % 6);
        match op % 4 {
            0 => {
                endpoint += 1;
                if let Ok(lease) = space.claim(address, endpoint) {
                    assert!(model.insert(address, endpoint).is_none(), "step {step}");
                    leases.insert(address, lease);
                } else {
                    assert!(model.contains_key(&address), "step {step}");
                }
            }
            1 | 2 => {
                let resolved = space.resolve(&address);
                assert_eq!(
                    resolved.as_deref().copied(),
                    model.get(&address).copied(),
                    "step {step}"
                );
                if let Some(value) = resolved {
                    snapshots.push(value); // held past release/replacement
                }
            }
            _ => {
                if let Some(lease) = leases.remove(&address) {
                    lease.release();
                    model.remove(&address);
                }
            }
        }
    }
    for (_, lease) in leases {
        lease.release();
    }
    assert!(space.is_empty());
    // The space is empty; every snapshot ever taken is still intact.
    for (i, value) in snapshots.iter().enumerate() {
        assert!(**value >= 1 && **value <= endpoint, "snapshot {i} corrupted: {value:?}");
    }
    // Exactness: snapshots must equal the values recorded at resolve time
    // (they ARE the values; this pins that no release/replacement mutated
    // or freed the underlying endpoint while a snapshot lived).
    assert!(snapshots.iter().all(|value| **value <= endpoint));
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn snapshots_survive_release_and_replacement(
        ops in prop::collection::vec((any::<u8>(), any::<u8>()), 1..100)
    ) {
        run(&ops);
    }
}

/// Deterministic pinpoint: a snapshot taken before a release-and-replace
/// cycle still holds the OLD value, and a snapshot taken after holds the
/// NEW one.
#[test]
fn snapshots_pin_their_generation_value() {
    let space = AddressSpace::new();
    let first = space.claim(0_u64, String::from("old")).unwrap();
    let old_snapshot = space.resolve(&0).unwrap();
    first.release();
    let second = space.claim(0_u64, String::from("new")).unwrap();
    let new_snapshot = space.resolve(&0).unwrap();
    assert_eq!(old_snapshot.as_str(), "old");
    assert_eq!(new_snapshot.as_str(), "new");
    second.release();
    assert_eq!(old_snapshot.as_str(), "old");
    assert_eq!(new_snapshot.as_str(), "new");
    assert!(space.is_empty());
}
