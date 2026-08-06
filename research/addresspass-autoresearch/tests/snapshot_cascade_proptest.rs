//! Snapshot-under-cascade proptest: resolve snapshots taken from
//! chain-INTERNAL addresses must survive a whole-chain cascade (the
//! cascade releases every registration through endpoint drops), and a
//! snapshot must pin its value across release + reclaim churn at the
//! same base. Combines the snapshot-validity and cascade lanes in one
//! workload — neither lane alone covers the interplay.
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
        // A resolved snapshot must not own the chain below it.
        Self {
            value: self.value,
            next: None,
        }
    }
}

/// Build a chain of `depth` nested leases at addresses
/// `base .. base + depth`; returns the outermost (top) lease.
fn build_chain(space: &AddressSpace<u64, Link>, base: u64, depth: u64) -> Lease<u64, Link> {
    let mut next: Option<Box<Lease<u64, Link>>> = None;
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
        next = Some(Box::new(lease));
    }
    *next.expect("depth >= 1")
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// Snapshots taken from every address of a chain — including
    /// chain-internal (non-top) links — survive the whole-chain cascade
    /// intact, and the space drains exactly.
    #[test]
    fn chain_internal_snapshots_survive_whole_chain_cascade(
        depth in 1_u64..64,
        chain_count in 1_usize..5,
        snapshot_every in 1_usize..8,
    ) {
        let space = AddressSpace::<u64, Link>::new();
        let mut chains = Vec::new();
        for chain in 0..chain_count {
            let base = (chain as u64) * 1_000;
            let top = build_chain(&space, base, depth);
            chains.push((base, top));
        }
        assert_eq!(space.len(), chain_count * depth as usize);

        // Snapshot every `snapshot_every`-th address of every chain,
        // including internal links. Each snapshot must pin its value.
        let mut snapshots = Vec::new();
        for (base, _) in &chains {
            for offset in (0..depth).step_by(snapshot_every) {
                let address = base + offset;
                let snapshot = space.resolve(&address).expect("chain address must resolve");
                assert_eq!(snapshot.value, address);
                snapshots.push((address, snapshot));
            }
        }

        // Cascade every chain: all registrations released through drops.
        for (base, top) in chains {
            top.release();
            for offset in 0..depth {
                assert!(space.resolve(&(base + offset)).is_none(), "chain must drain");
            }
        }
        assert!(space.is_empty());

        // Every held snapshot stayed intact with its pinned value.
        for (address, snapshot) in snapshots {
            assert_eq!(snapshot.value, address, "snapshot of {address} corrupted");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// A snapshot pins its value across release + reclaim at the same
    /// base: the old snapshot keeps the OLD chain's value while a fresh
    /// chain replaces it, and both coexist exactly.
    #[test]
    fn snapshot_pins_value_across_release_and_reclaim(
        depth in 1_u64..32,
        rounds in 1_usize..8,
    ) {
        let space = AddressSpace::<u64, Link>::new();
        let base = 10_000_u64;
        let mut pinned = Vec::new();

        for round in 0..rounds {
            let top = build_chain(&space, base, depth);
            assert_eq!(space.len(), depth as usize, "round {round}: build");
            // Snapshot the top and an internal link of THIS generation.
            let top_snapshot = space.resolve(&base).expect("top must resolve");
            let internal = space
                .resolve(&(base + depth / 2))
                .expect("internal link must resolve");
            assert_eq!(top_snapshot.value, base, "round {round}: top");
            assert_eq!(internal.value, base + depth / 2, "round {round}: internal");
            pinned.push((base, top_snapshot, internal));
            top.release();
            assert!(space.is_empty(), "round {round}: drain");
        }

        // Every pinned snapshot from every generation kept its value.
        for (address, top_snapshot, internal) in pinned {
            assert_eq!(top_snapshot.value, address);
            assert_eq!(internal.value, address + depth / 2);
        }
        assert!(space.is_empty());
    }
}
