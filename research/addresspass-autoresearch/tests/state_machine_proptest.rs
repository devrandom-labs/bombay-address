//! Randomized state-machine verification: proptest generates operation
//! histories; the SUT and the independent `ReferenceModel` must agree on
//! every observable result after every step.
//!
//! Excluded from Miri (512 cases × 200 ops is a native-speed workload).
#![cfg(not(miri))]

use addresspass::{AddressInUse, AddressSpace, Lease};
use addresspass_autoresearch::ReferenceModel;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

/// Model-side tracking for a live lease (see `sequential_model.rs`).
struct Tracked {
    lease: Lease<u64, u64>,
    address: u64,
    generation: u64,
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Claim(u8),
    /// Release the live lease selected by index modulo the live count.
    Release(usize),
    Resolve(u8),
}

fn op_strategy() -> BoxedStrategy<Op> {
    prop_oneof![
        3 => (0u8..16).prop_map(Op::Claim),
        2 => (0usize..64).prop_map(Op::Release),
        4 => (0u8..16).prop_map(Op::Resolve),
    ]
    .boxed()
}

fn run_history(ops: &[Op]) {
    let space = AddressSpace::new();
    let mut model = ReferenceModel::new();
    let mut leases: Vec<Option<Tracked>> = Vec::new();
    let mut endpoint = 0_u64;
    for (step, op) in ops.iter().enumerate() {
        match *op {
            Op::Claim(address) => {
                endpoint += 1;
                let result = space.claim(u64::from(address), endpoint);
                match (result, model.claim(u64::from(address), endpoint)) {
                    (Ok(lease), Some(generation)) => leases.push(Some(Tracked {
                        lease,
                        address: u64::from(address),
                        generation,
                    })),
                    (Err(AddressInUse(returned)), None) => {
                        assert_eq!(returned, u64::from(address), "step {step}");
                    }
                    (Ok(_), None) => panic!("step {step}: SUT claimed owned address"),
                    (Err(_), Some(_)) => panic!("step {step}: SUT refused free address"),
                }
            }
            Op::Release(pick) => {
                let live: Vec<usize> = leases
                    .iter()
                    .enumerate()
                    .filter_map(|(i, slot)| slot.as_ref().map(|_| i))
                    .collect();
                if live.is_empty() {
                    continue;
                }
                let index = live[pick % live.len()];
                let tracked = leases[index].take().unwrap();
                model.release(tracked.address, tracked.generation);
                drop(tracked.lease);
            }
            Op::Resolve(address) => {
                assert_eq!(
                    space.resolve(&u64::from(address)),
                    model.resolve(u64::from(address)),
                    "step {step}: resolve({address}) diverged"
                );
            }
        }
        assert_eq!(space.len(), model.len(), "step {step}: len diverged");
        assert_eq!(space.is_empty(), model.is_empty(), "step {step}");
    }
    // Drain: every still-live lease releases its exact registration.
    for slot in &mut leases {
        if let Some(tracked) = slot.take() {
            model.release(tracked.address, tracked.generation);
            drop(tracked.lease);
        }
    }
    assert!(space.is_empty(), "drained space must be empty");
    assert!(model.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_histories_match_reference_model(
        ops in prop::collection::vec(op_strategy(), 1..200)
    ) {
        run_history(&ops);
    }

    /// Adversarial variant: histories biased toward a single hot address,
    /// maximizing contention on one registration's lifecycle.
    #[test]
    fn hot_address_histories_match_reference_model(
        ops in prop::collection::vec(
            prop_oneof![
                6 => Just(Op::Claim(0)),
                3 => (0usize..8).prop_map(Op::Release),
                4 => Just(Op::Resolve(0)),
                1 => (0u8..4).prop_map(Op::Claim),
            ],
            1..200
        )
    ) {
        run_history(&ops);
    }
}
