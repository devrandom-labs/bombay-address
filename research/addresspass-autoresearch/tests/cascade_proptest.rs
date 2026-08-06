//! Cascade proptest: multiple nested lease chains (each endpoint owns the
//! next lease down its chain) with random whole-chain cascades and
//! resolves. Releasing a chain's top cascades through every link below
//! it; the model must track each cascade exactly.
//!
//! (Only the top lease of a chain is reachable through the public API —
//! inner leases live inside endpoints — so cascades are whole-chain by
//! construction.)
//!
//! Excluded from Miri (case counts are a native-speed workload).
#![cfg(not(miri))]

use addresspass::{AddressSpace, Lease};
use proptest::prelude::*;

/// One chain link: value for identity, plus the next lease down.
struct Link {
    value: u64,
    #[expect(
        dead_code,
        reason = "the field is exercised through drop (cascade recursion), never read"
    )]
    next: Option<Box<Lease<u64, Link>>>,
}

impl Clone for Link {
    fn clone(&self) -> Self {
        Self {
            value: self.value,
            next: None,
        }
    }
}

/// Build a chain of `depth` nested leases at addresses
/// `base .. base + depth`; returns the outermost (top) lease and the
/// owned addresses.
fn build_chain(space: &AddressSpace<u64, Link>, base: u64, depth: u64) -> (Lease<u64, Link>, Vec<u64>) {
    let mut next: Option<Box<Lease<u64, Link>>> = None;
    let mut addresses = Vec::new();
    for offset in (0..depth).rev() {
        let address = base + offset;
        let lease = space
            .claim(
                address,
                Link {
                    value: address,
                    next: next.take(),
                },
            )
            .unwrap();
        addresses.push(address);
        next = Some(Box::new(lease));
    }
    (*next.unwrap(), addresses)
}

/// A live chain slot: the top lease plus the owned addresses, `None`
/// after the chain cascaded.
type ChainSlot = Option<(Lease<u64, Link>, Vec<u64>)>;

fn run(depth: u64, chain_count: usize, ops: &[(u8, u8)]) {
    let space = AddressSpace::<u64, Link>::new();
    let chain_count = chain_count.max(1);
    let depth = depth.max(1);
    let mut model: std::collections::BTreeMap<u64, u64> = Default::default();
    let mut chains: Vec<ChainSlot> = Vec::new();
    for chain in 0..chain_count {
        let base = (chain as u64) * 1_000;
        let (top, addresses) = build_chain(&space, base, depth);
        for &address in &addresses {
            model.insert(address, address);
        }
        chains.push(Some((top, addresses)));
    }
    assert_eq!(space.len(), model.len());

    for (step, (pick_chain, op)) in ops.iter().enumerate() {
        let index = usize::from(*pick_chain) % chains.len();
        if op % 2 == 0 {
            // Cascade this chain (no-op if already cascaded).
            if let Some((top, addresses)) = chains[index].take() {
                top.release();
                for &address in &addresses {
                    model.remove(&address);
                }
            }
        } else {
            // Resolve an address from this chain's range.
            let addresses: Vec<u64> = (0..depth)
                .map(|offset| (index as u64) * 1_000 + offset)
                .collect();
            let address = addresses[usize::from(*op) % addresses.len()];
            let resolved = space.resolve(&address);
            assert_eq!(
                resolved.as_ref().map(|l| l.value),
                model.get(&address).copied(),
                "step {step}: resolve({address}) diverged"
            );
        }
        assert_eq!(space.len(), model.len(), "step {step}: len diverged");
    }
    for slot in &mut chains {
        if let Some((top, _)) = slot.take() {
            top.release();
        }
    }
    assert!(space.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    #[test]
    fn cascade_chains_drain_exactly(
        depth in 1_u64..64,
        chain_count in 1_usize..5,
        ops in prop::collection::vec((any::<u8>(), any::<u8>()), 0..40)
    ) {
        run(depth, chain_count, &ops);
    }
}

/// Deterministic pinpoint: releasing the top of a 3-chain removes all
/// three registrations.
#[test]
fn top_release_cascades_whole_chain() {
    let space = AddressSpace::<u64, Link>::new();
    let (top, _) = build_chain(&space, 10, 3);
    assert_eq!(space.len(), 3);
    top.release();
    assert!(space.is_empty());
}

/// Deterministic pinpoint: a build whose range collides with a live chain
/// aborts mid-way; the partial chain is released through the rejected
/// endpoint's drop cascade, and the live chain is left untouched.
#[test]
fn colliding_build_aborts_and_releases_partial_chain() {
    let space = AddressSpace::<u64, Link>::new();
    let (top, _addresses) = build_chain(&space, 0, 3); // owns 0, 1, 2
    assert_eq!(space.len(), 3);

    // Attempt a deeper build at the same base: claims 5, 4, then 3
    // succeed; the claim of 2 collides and the partial chain (3, 4, 5)
    // must cascade back out.
    let mut next: Option<Box<Lease<u64, Link>>> = None;
    let mut claimed: Vec<u64> = Vec::new();
    let mut aborted = false;
    for offset in (0..6).rev() {
        let address = offset;
        match space.claim(address, Link { value: address, next: next.take() }) {
            Ok(lease) => {
                claimed.push(address);
                next = Some(Box::new(lease));
            }
            Err(addresspass::AddressInUse(returned)) => {
                assert_eq!(returned, address);
                aborted = true;
                break;
            }
        }
    }
    assert!(aborted, "colliding build must abort");
    drop(next); // rejected endpoint's partial chain cascades on drop

    // Exactly the original three registrations remain.
    assert_eq!(space.len(), 3);
    for address in 0..3 {
        assert!(
            space.resolve(&address).is_some(),
            "live chain lost {address}"
        );
    }
    for address in 3..6 {
        assert!(
            space.resolve(&address).is_none(),
            "partial chain leaked {address}"
        );
    }
    assert_eq!(claimed, vec![5, 4, 3]);
    drop(top);
    assert!(space.is_empty());
}
