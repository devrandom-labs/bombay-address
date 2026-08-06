//! Proptest state machine over STRING addresses — exercises the custom
//! hasher's chunked `write` path (multi-chunk folding, 0xff string
//! terminator, length-prefix writes) that `u64` addresses never touch,
//! including engineered zero-padding collision pairs.
//!
//! Excluded from Miri (case counts are a native-speed workload).
#![cfg(not(miri))]

use addresspass::{AddressInUse, AddressSpace, Lease};
use proptest::prelude::*;

/// Model state for string addresses: address → endpoint, plus the lease
/// handles keyed by address. (A full `ReferenceModel` is u64-keyed; the
/// string-keyed oracle is a plain BTreeMap — same independent-oracle
/// discipline, no shared code with the SUT.)
struct State {
    model: std::collections::BTreeMap<String, u64>,
    leases: std::collections::BTreeMap<String, Lease<String, u64>>,
}

fn run(ops: &[(String, u8)]) {
    let space = AddressSpace::<String, u64>::new();
    let mut state = State {
        model: std::collections::BTreeMap::new(),
        leases: std::collections::BTreeMap::new(),
    };
    let mut endpoint = 0_u64;
    for (step, (address, op)) in ops.iter().enumerate() {
        match op % 3 {
            0 => {
                endpoint += 1;
                match space.claim(address.clone(), endpoint) {
                    Ok(lease) => {
                        assert!(
                            state.model.insert(address.clone(), endpoint).is_none(),
                            "step {step}: SUT claimed an owned address"
                        );
                        state.leases.insert(address.clone(), lease);
                    }
                    Err(AddressInUse(returned)) => {
                        assert_eq!(&returned, address, "step {step}");
                        assert!(
                            state.model.contains_key(address),
                            "step {step}: SUT refused a free address"
                        );
                    }
                }
            }
            1 => {
                assert_eq!(
                    space.resolve(address),
                    state.model.get(address).copied(),
                    "step {step}: resolve({address:?}) diverged"
                );
            }
            _ => {
                if let Some(lease) = state.leases.remove(address) {
                    lease.release();
                    let removed = state.model.remove(address);
                    assert!(removed.is_some(), "step {step}");
                }
                assert_eq!(space.resolve(address), None, "step {step}");
            }
        }
        assert_eq!(space.len(), state.model.len(), "step {step}: len diverged");
    }
    for (_, lease) in state.leases {
        lease.release();
    }
    assert!(space.is_empty());
}

/// Address strategy: short strings over a tiny alphabet (forces natural
/// collisions and multi-chunk `write` folding), plus zero-padded variants
/// (the `write` chunk loop pads the final short chunk with zeros, so
/// trailing-zero extensions are the adversarial case).
fn address_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => prop::collection::vec(prop::sample::select(vec![b'a', b'b', 0u8, 0xff]), 0..24usize)
            .prop_map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
        1 => (0usize..16).prop_map(|zeros| format!("a{}", "\0".repeat(zeros))),
        1 => Just(String::new()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn string_address_histories_match_oracle(
        ops in prop::collection::vec((address_strategy(), any::<u8>()), 1..100)
    ) {
        run(&ops);
    }
}

/// Zero-extended strings that collide in the custom hasher's chunk folding
/// (final chunk zero-padding) must still coexist as DISTINCT keys.
#[test]
fn zero_padding_collision_pairs_coexist() {
    let space = AddressSpace::new();
    let short = String::from("a");
    let padded = format!("a{}", "\0".repeat(7)); // same first chunk after padding
    let first = space.claim(short.clone(), 1_u64).unwrap();
    let second = space.claim(padded.clone(), 2_u64).unwrap();
    assert_eq!(space.resolve(&short), Some(1));
    assert_eq!(space.resolve(&padded), Some(2));
    assert_eq!(space.len(), 2);
    first.release();
    assert_eq!(space.resolve(&short), None);
    assert_eq!(space.resolve(&padded), Some(2));
    second.release();
    assert!(space.is_empty());
}
