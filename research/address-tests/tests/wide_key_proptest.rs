//! Wide and composite key types: `(u64, u64)` tuples (two `write_u64`
//! folds — order sensitivity between parts) and `u128` (default
//! `write_u128` → byte `write` chunks). These exercise hasher paths that
//! plain `u64` and string keys do not.
//!
//! Excluded from Miri (case counts are a native-speed workload).
#![cfg(not(miri))]

use bombay_address::{AddressInUse, AddressSpace, Lease};
use proptest::prelude::*;

/// Run a history over a composite key domain against a BTreeMap oracle.
fn run(ops: &[((u64, u64), u8)]) {
    let space = AddressSpace::<(u64, u64), u64>::new();
    let mut model: std::collections::BTreeMap<(u64, u64), u64> = Default::default();
    let mut leases: std::collections::BTreeMap<(u64, u64), Lease<(u64, u64), u64>> =
        Default::default();
    let mut endpoint = 0_u64;
    for (step, (address, op)) in ops.iter().enumerate() {
        let address = (address.0 % 4, address.1 % 4);
        match op % 3 {
            0 => {
                endpoint += 1;
                match space.claim(address, endpoint) {
                    Ok(lease) => {
                        assert!(model.insert(address, endpoint).is_none(), "step {step}");
                        leases.insert(address, lease);
                    }
                    Err(AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(model.contains_key(&address), "step {step}");
                    }
                }
            }
            1 => {
                assert_eq!(
                    space.resolve(&address),
                    model.get(&address).copied(),
                    "step {step}"
                );
            }
            _ => {
                if let Some(lease) = leases.remove(&address) {
                    lease.release();
                    model.remove(&address);
                }
                assert_eq!(space.resolve(&address), None, "step {step}");
            }
        }
        assert_eq!(space.len(), model.len(), "step {step}");
    }
    for (_, lease) in leases {
        lease.release();
    }
    assert!(space.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn tuple_address_histories_match_oracle(
        ops in prop::collection::vec(((any::<u64>(), any::<u64>()), any::<u8>()), 1..100)
    ) {
        run(&ops);
    }
}

/// Order sensitivity: `(a, b)` and `(b, a)` must be distinct registrations
/// even though they fold the same two words into the hasher.
#[test]
fn tuple_parts_are_order_sensitive() {
    let space = AddressSpace::new();
    let ab = space.claim((1_u64, 2_u64), 12_u64).unwrap();
    let ba = space.claim((2_u64, 1_u64), 21_u64).unwrap();
    assert_eq!(space.resolve(&(1, 2)), Some(12));
    assert_eq!(space.resolve(&(2, 1)), Some(21));
    assert_eq!(space.len(), 2);
    ab.release();
    assert_eq!(space.resolve(&(1, 2)), None);
    assert_eq!(space.resolve(&(2, 1)), Some(21));
    ba.release();
    assert!(space.is_empty());
}

/// u128 keys: values spanning both 64-bit halves (exercises the chunked
/// `write` fallback for 128-bit integers).
#[test]
fn u128_addresses_spanning_both_halves() {
    let space = AddressSpace::new();
    let addresses = [
        0_u128,
        1,
        u64::MAX as u128,
        (u64::MAX as u128) + 1,
        u128::MAX - 1,
        u128::MAX,
    ];
    let mut leases = Vec::new();
    for (i, &address) in addresses.iter().enumerate() {
        leases.push(space.claim(address, i as u64).unwrap());
    }
    assert_eq!(space.len(), addresses.len());
    for (i, &address) in addresses.iter().enumerate() {
        assert_eq!(space.resolve(&address), Some(i as u64));
    }
    for lease in leases {
        lease.release();
    }
    assert!(space.is_empty());
}
