//! Shared support code for the bombay-address adversarial test campaign.
//!
//! This crate contains ONLY tests and test infrastructure. It never modifies
//! production code; it exercises `bombay-address` as an external dependency.

/// A deterministic xorshift64* PRNG (Vigna, TOMS 2015) used by the
/// exhaustive explorers and the corpus replay harness so every campaign run
/// is bit-for-bit reproducible without external crates.
pub struct XorShift64Star(u64);

impl XorShift64Star {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        // xorshift cannot start from a zero state.
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `0..bound` (bound must be non-zero) via multiply-shift.
    pub fn below(&mut self, bound: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(bound)) >> 64) as u64
    }
}

/// A reference model of the documented bombay-address semantics, implemented
/// independently (a plain `BTreeMap`, no shared code) to serve as the test
/// oracle.
///
/// Semantics encoded:
/// - at most one live registration owns an address;
/// - a claim on an owned address fails with the address returned;
/// - releasing generation g removes the registration only if the live
///   registration has exactly generation g (stale releases are no-ops);
/// - generations are strictly increasing, starting at 1, one per successful
///   claim.
#[derive(Default)]
pub struct ReferenceModel {
    live: std::collections::BTreeMap<u64, (u64, u64)>,
    next_generation: u64,
}

impl ReferenceModel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            live: std::collections::BTreeMap::new(),
            next_generation: 1,
        }
    }

    /// Returns the generation of the new lease, or `None` if the address is
    /// already owned.
    pub fn claim(&mut self, address: u64, endpoint: u64) -> Option<u64> {
        if self.live.contains_key(&address) {
            return None;
        }
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .expect("model generation overflow");
        self.live.insert(address, (generation, endpoint));
        Some(generation)
    }

    pub fn release(&mut self, address: u64, generation: u64) {
        if self.live.get(&address).is_some_and(|(g, _)| *g == generation) {
            self.live.remove(&address);
        }
    }

    #[must_use]
    pub fn resolve(&self, address: u64) -> Option<u64> {
        self.live.get(&address).map(|(_, endpoint)| *endpoint)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
}

/// Byte-stream fuzzer entry: decodes an operation history from `data` and
/// replays it against the SUT and the reference model, panicking on any
/// divergence. Shared by the libFuzzer target and the deterministic replay
/// harness.
///
/// Encoding: two bytes per operation. `op = b0 % 4` selects claim (0),
/// release (1), resolve (2), or len-check (3); `address = (b0 / 4) % 16`;
/// `b1` salts the claimed endpoint value.
pub fn fuzz_entry(data: &[u8]) {
    let space = bombay_address::AddressSpace::<u64, u64>::new();
    let mut model = ReferenceModel::new();
    // Per-address live lease slots: (generation, lease).
    let mut leases: [Option<(u64, bombay_address::Lease<u64, u64>)>; 16] =
        [None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None];
    for pair in data.chunks_exact(2) {
        let address = u64::from(pair[0] / 4) % 16;
        match pair[0] % 4 {
            0 => {
                let endpoint = u64::from(pair[1]) + 1;
                match space.claim(address, endpoint) {
                    Ok(lease) => {
                        let generation = model
                            .claim(address, endpoint)
                            .expect("SUT claimed an address the model owns");
                        leases[address as usize] = Some((generation, lease));
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address);
                        assert!(
                            model.claim(address, endpoint).is_none(),
                            "model claimed an address the SUT owns"
                        );
                    }
                }
            }
            1 => {
                if let Some((generation, lease)) = leases[address as usize].take() {
                    model.release(address, generation);
                    drop(lease);
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&address).as_deref().copied(),
                    model.resolve(address)
                );
            }
            3 => {
                assert_eq!(space.len(), model.len());
            }
            _ => unreachable!(),
        }
    }
    for (index, slot) in leases.iter_mut().enumerate() {
        if let Some((generation, lease)) = slot.take() {
            model.release(index as u64, generation);
            drop(lease);
        }
    }
    assert!(space.is_empty());
}

/// Variant of [`fuzz_entry`] whose addresses all collide in one hash
/// bucket, stressing collision exclusion under fuzzed histories.
pub fn fuzz_entry_colliding(data: &[u8]) {
    use std::hash::{Hash, Hasher};

    #[derive(Clone, PartialEq, Eq, Debug)]
    struct Colliding(u64);
    impl Hash for Colliding {
        fn hash<H: Hasher>(&self, state: &mut H) {
            state.write_u64(0);
        }
    }

    let space = bombay_address::AddressSpace::<Colliding, u64>::new();
    let mut model = ReferenceModel::new();
    let mut leases: [Option<(u64, bombay_address::Lease<Colliding, u64>)>; 16] =
        [None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None];
    for pair in data.chunks_exact(2) {
        let address = u64::from(pair[0] / 4) % 16;
        match pair[0] % 4 {
            0 => {
                let endpoint = u64::from(pair[1]) + 1;
                match space.claim(Colliding(address), endpoint) {
                    Ok(lease) => {
                        let generation = model
                            .claim(address, endpoint)
                            .expect("SUT claimed an address the model owns");
                        leases[address as usize] = Some((generation, lease));
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, Colliding(address));
                        assert!(model.claim(address, endpoint).is_none());
                    }
                }
            }
            1 => {
                if let Some((generation, lease)) = leases[address as usize].take() {
                    model.release(address, generation);
                    drop(lease);
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&Colliding(address)).as_deref().copied(),
                    model.resolve(address)
                );
            }
            3 => {
                assert_eq!(space.len(), model.len());
            }
            _ => unreachable!(),
        }
    }
    for (index, slot) in leases.iter_mut().enumerate() {
        if let Some((generation, lease)) = slot.take() {
            model.release(index as u64, generation);
            drop(lease);
        }
    }
    assert!(space.is_empty());
}

/// String-address variant of [`fuzz_entry`]: decodes operations whose
/// addresses are short strings over a 4-letter alphabet with zero-padding
/// variants, exercising the custom hasher's chunked `write` path.
///
/// Encoding: three bytes per operation. `op = b0 % 4` (claim, release,
/// resolve, len-check); the address is derived from `b1` (length 0..8) and
/// `b2` (alphabet selector plus zero-pad flag).
pub fn fuzz_entry_strings(data: &[u8]) {
    let space = bombay_address::AddressSpace::<String, u64>::new();
    let mut model: std::collections::BTreeMap<String, u64> = Default::default();
    let mut leases: std::collections::BTreeMap<String, bombay_address::Lease<String, u64>> =
        Default::default();
    let mut endpoint = 0_u64;
    for (step, triple) in data.chunks_exact(3).enumerate() {
        let length = usize::from(triple[1] % 8);
        let mut address = String::new();
        for i in 0..length {
            let letter = b"ab\0\xff"[usize::from(triple[2].wrapping_add(i as u8)) % 4];
            address.push(letter as char);
        }
        if triple[2] % 2 == 1 {
            address.push('\0'); // zero-pad extension: hasher collision case
        }
        match triple[0] % 4 {
            0 => {
                endpoint += 1;
                match space.claim(address.clone(), endpoint) {
                    Ok(lease) => {
                        assert!(
                            model.insert(address.clone(), endpoint).is_none(),
                            "step {step}: SUT claimed an owned address"
                        );
                        leases.insert(address, lease);
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(model.contains_key(&address), "step {step}");
                    }
                }
            }
            1 => {
                if let Some(lease) = leases.remove(&address) {
                    lease.release();
                    model.remove(&address);
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&address).as_deref().copied(),
                    model.get(&address).copied(),
                    "step {step}"
                );
            }
            3 => {
                assert_eq!(space.len(), model.len(), "step {step}");
            }
            _ => unreachable!(),
        }
    }
    for (_, lease) in leases {
        lease.release();
    }
    assert!(space.is_empty());
}

/// Wide-address variant of [`fuzz_entry`]: 1-byte operations over 64
/// addresses, maximizing the number of simultaneously live registrations
/// the fuzzer can reach.
///
/// Encoding: one byte per operation. `op = b % 4` (claim, release,
/// resolve, len-check); `address = (b / 4) % 64`; claimed endpoints are a
/// running counter (unique, non-zero).
pub fn fuzz_entry_wide(data: &[u8]) {
    let space = bombay_address::AddressSpace::<u64, u64>::new();
    let mut model = ReferenceModel::new();
    let mut leases: std::collections::BTreeMap<u64, (u64, bombay_address::Lease<u64, u64>)> =
        Default::default();
    let mut endpoint = 0_u64;
    for (step, &byte) in data.iter().enumerate() {
        let address = u64::from(byte / 4) % 64;
        match byte % 4 {
            0 => {
                endpoint += 1;
                match space.claim(address, endpoint) {
                    Ok(lease) => {
                        let generation = model
                            .claim(address, endpoint)
                            .unwrap_or_else(|| panic!("step {step}: SUT claimed owned"));
                        leases.insert(address, (generation, lease));
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(model.claim(address, endpoint).is_none(), "step {step}");
                    }
                }
            }
            1 => {
                if let Some((generation, lease)) = leases.remove(&address) {
                    model.release(address, generation);
                    drop(lease);
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&address).as_deref().copied(),
                    model.resolve(address),
                    "step {step}"
                );
            }
            3 => {
                assert_eq!(space.len(), model.len(), "step {step}");
            }
            _ => unreachable!(),
        }
    }
    for (address, (generation, lease)) in leases {
        model.release(address, generation);
        drop(lease);
    }
    assert!(space.is_empty());
}

/// Lease table used by the fuzz entries: address → (generation, lease).
pub type LeaseTable = std::collections::BTreeMap<u64, (u64, bombay_address::Lease<u64, u64>)>;

/// Two-space isolation variant: operations are routed by the top bit to
/// one of two independent address spaces; each must behave as if the
/// other did not exist (no shared state between instances).
///
/// Encoding: two bytes per operation. `space = b0 >> 7`; `op = (b0 >> 5)
/// % 3` (claim, release, resolve); `address = (b0 & 0x1f) % 8`; `b1`
/// salts claimed endpoints.
pub fn fuzz_entry_isolation(data: &[u8]) {
    let spaces = [
        bombay_address::AddressSpace::<u64, u64>::new(),
        bombay_address::AddressSpace::<u64, u64>::new(),
    ];
    let mut models = [ReferenceModel::new(), ReferenceModel::new()];
    let mut leases: [LeaseTable; 2] = [Default::default(), Default::default()];
    let mut endpoint = 0_u64;
    for (step, pair) in data.chunks_exact(2).enumerate() {
        let which = usize::from(pair[0] >> 7);
        let address = u64::from(pair[0] & 0x1f) % 8;
        match (pair[0] >> 5) % 3 {
            0 => {
                endpoint += 1;
                match spaces[which].claim(address, endpoint) {
                    Ok(lease) => {
                        let generation = models[which]
                            .claim(address, endpoint)
                            .unwrap_or_else(|| panic!("step {step}: SUT claimed owned"));
                        leases[which].insert(address, (generation, lease));
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(models[which].claim(address, endpoint).is_none(), "step {step}");
                    }
                }
            }
            1 => {
                if let Some((generation, lease)) = leases[which].remove(&address) {
                    models[which].release(address, generation);
                    drop(lease);
                }
            }
            _ => {
                // Check BOTH spaces: the unrouted one must be unaffected
                // by everything routed to its peer.
                for (side, (space, model)) in spaces.iter().zip(models.iter()).enumerate() {
                    assert_eq!(
                        space.resolve(&address).as_deref().copied(),
                        model.resolve(address),
                        "step {step}: space {side} diverged"
                    );
                    assert_eq!(space.len(), model.len(), "step {step}: space {side}");
                }
            }
        }
    }
    for (which, leases) in leases.into_iter().enumerate() {
        for (address, (generation, lease)) in leases {
            models[which].release(address, generation);
            drop(lease);
        }
    }
    assert!(spaces[0].is_empty() && spaces[1].is_empty());
}

/// Cascade variant of [`fuzz_entry`]: nested lease chains (each endpoint
/// owns the next lease down its chain) built at fixed bases, with random
/// builds — including builds that collide with a live chain and must
/// abort, releasing the partial chain through a drop cascade — whole-chain
/// cascades, resolves, and len checks, model-checked per step.
///
/// Encoding: two bytes per operation. `op = b0 % 4` selects build (0),
/// cascade (1), resolve (2), or len-check (3); `slot = (b0 / 4) % 4`
/// selects one of four chain bases (`slot * 16`); `b1` gives the build
/// depth (`1 + b1 % 8`) or the resolve offset within the slot's range.
pub fn fuzz_entry_cascade(data: &[u8]) {
    /// One chain link: value for identity, plus the next lease down.
    struct Link {
        value: u64,
        #[expect(
            dead_code,
            reason = "the field is exercised through drop (cascade recursion), never read"
        )]
        next: Option<Box<bombay_address::Lease<u64, Link>>>,
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

    const SLOT_BASE: u64 = 16;
    const MAX_DEPTH: u64 = 8;

    let space = bombay_address::AddressSpace::<u64, Link>::new();
    // Model: address -> endpoint value (the address itself, mirroring the
    // deterministic cascade tests).
    let mut model: std::collections::BTreeMap<u64, u64> = Default::default();
    /// Per-slot live chain: the top lease plus every address it owns.
    type ChainSlot = Option<(bombay_address::Lease<u64, Link>, Vec<u64>)>;
    let mut chains: [ChainSlot; 4] = [None, None, None, None];
    for (step, pair) in data.chunks_exact(2).enumerate() {
        let slot = usize::from(pair[0] / 4) % 4;
        let base = (slot as u64) * SLOT_BASE;
        match pair[0] % 4 {
            0 => {
                let depth = 1 + u64::from(pair[1]) % MAX_DEPTH;
                // Claim deepest-first, exactly like the deterministic
                // builders; on the first collision the partial chain is
                // released by the rejected endpoint's drop cascade.
                let mut next: Option<Box<bombay_address::Lease<u64, Link>>> = None;
                let mut claimed: Vec<u64> = Vec::new();
                let mut aborted = false;
                for offset in (0..depth).rev() {
                    let address = base + offset;
                    match space.claim(address, Link { value: address, next: next.take() }) {
                        Ok(lease) => {
                            assert!(
                                model.insert(address, address).is_none(),
                                "step {step}: SUT claimed an address the model owns"
                            );
                            claimed.push(address);
                            next = Some(Box::new(lease));
                        }
                        Err(bombay_address::AddressInUse(returned)) => {
                            assert_eq!(returned, address, "step {step}");
                            assert!(
                                model.contains_key(&address),
                                "step {step}: SUT rejected an address the model owns"
                            );
                            aborted = true;
                            break;
                        }
                    }
                }
                if aborted {
                    // The rejected endpoint (with the partial chain in its
                    // `next`) was dropped by `claim`; every address claimed
                    // this round has been released. Mirror the release.
                    for address in claimed {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: partial cascade released an address the model did not own"
                        );
                    }
                } else {
                    let top = *next.expect("depth >= 1");
                    chains[slot] = Some((top, claimed));
                }
            }
            1 => {
                if let Some((top, addresses)) = chains[slot].take() {
                    top.release();
                    for address in addresses {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: cascade released an address the model did not own"
                        );
                    }
                }
            }
            2 => {
                let address = base + u64::from(pair[1]) % MAX_DEPTH;
                assert_eq!(
                    space.resolve(&address).map(|l| l.value),
                    model.get(&address).copied(),
                    "step {step}: resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    for slot in &mut chains {
        if let Some((top, addresses)) = slot.take() {
            top.release();
            for address in addresses {
                model.remove(&address);
            }
        }
    }
    assert!(space.is_empty() && model.is_empty());
}

/// Reentrant-`Drop` variant of [`fuzz_entry`]: an endpoint whose `Drop`
/// claims a NEW registration in the same space and parks the spawned
/// lease in a shared bag (the spawned registration PERSISTS until the bag
/// is drained). Production drops removed endpoints outside the write
/// guard; this entry fuzzes that guarantee — the drop-time claim must not
/// deadlock, must not corrupt the table, and must leave the model exact.
///
/// Encoding: two bytes per operation. `op = b0 % 4` selects claim (0),
/// release (1), resolve (2), or len-check (3); `address = (b0 / 4) % 16`;
/// `b1` selects the spawn behavior (`b1 % 4 == 0` → no spawn, otherwise
/// spawn at `(b1 / 4) % 16`) and salts the endpoint value.
pub fn fuzz_entry_reentrant(data: &[u8]) {
    use std::sync::{Arc, Mutex};

    type Space = bombay_address::AddressSpace<u64, Box<Reentrant>>;
    type Bag = Arc<Mutex<Vec<bombay_address::Lease<u64, Box<Reentrant>>>>>;

    /// An endpoint whose `Drop` claims a new registration at `spawn`
    /// (one-shot: the spawned endpoint never spawns again).
    struct Reentrant {
        space: Arc<Space>,
        bag: Bag,
        value: u64,
        spawn: Option<u64>,
    }

    impl Clone for Reentrant {
        fn clone(&self) -> Self {
            // Resolved snapshots are inert: a clone never spawns.
            Self {
                space: Arc::clone(&self.space),
                bag: Arc::clone(&self.bag),
                value: self.value,
                spawn: None,
            }
        }
    }

    impl Drop for Reentrant {
        fn drop(&mut self) {
            // Runs after the write guard is released (documented design).
            // The nested claim may fail (address taken) — legal, mirror
            // it as a no-op. The spawned lease is parked so the spawned
            // registration persists; the entry drains the bag at the end.
            if let Some(spawn) = self.spawn {
                let spawned = Reentrant {
                    space: Arc::clone(&self.space),
                    bag: Arc::clone(&self.bag),
                    value: self.value,
                    spawn: None,
                };
                if let Ok(lease) = self.space.claim(spawn, Box::new(spawned)) {
                    self.bag.lock().expect("reentrant bag lock").push(lease);
                }
            }
        }
    }

    let space: Arc<Space> = Arc::new(bombay_address::AddressSpace::new());
    let bag: Bag = Arc::new(Mutex::new(Vec::new()));

    // Model: address -> (value, spawn). The spawn column is what the
    // endpoint at that address will do on release.
    let mut model: std::collections::BTreeMap<u64, (u64, Option<u64>)> = Default::default();
    let mut leases: [Option<bombay_address::Lease<u64, Box<Reentrant>>>; 16] =
        [None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None];
    for (step, pair) in data.chunks_exact(2).enumerate() {
        let address = u64::from(pair[0] / 4) % 16;
        match pair[0] % 4 {
            0 => {
                let value = u64::from(pair[1]) + 1;
                let spawn = if pair[1] % 4 == 0 {
                    None
                } else {
                    Some(u64::from(pair[1] / 4) % 16)
                };
                match space.claim(
                    address,
                    Box::new(Reentrant {
                        space: Arc::clone(&space),
                        bag: Arc::clone(&bag),
                        value,
                        spawn,
                    }),
                ) {
                    Ok(lease) => {
                        assert!(
                            model.insert(address, (value, spawn)).is_none(),
                            "step {step}: SUT claimed an address the model owns"
                        );
                        leases[address as usize] = Some(lease);
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(
                            model.contains_key(&address),
                            "step {step}: SUT rejected an address the model owns"
                        );
                        // The rejected endpoint is dropped by `claim`
                        // (outside the write guard); its `Drop` may spawn
                        // a new registration. Mirror it.
                        if let Some(spawn_address) = spawn {
                            model.entry(spawn_address).or_insert((value, None));
                        }
                    }
                }
            }
            1 => {
                if let Some(lease) = leases[address as usize].take() {
                    let (value, spawn) = model
                        .remove(&address)
                        .expect("step {step}: SUT released an address the model does not own");
                    drop(lease);
                    // The drop claimed `spawn` (if any) with the SAME value
                    // and parked the spawned lease. Mirror it.
                    if let Some(spawn_address) = spawn {
                        model.entry(spawn_address).or_insert((value, None));
                    }
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&address).map(|l| l.value),
                    model.get(&address).map(|(value, _)| *value),
                    "step {step}: resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    // Drain: release every live slot, then every spawned registration.
    for (index, slot) in leases.iter_mut().enumerate() {
        if let Some(lease) = slot.take() {
            let (value, spawn) = model
                .remove(&(index as u64))
                .expect("drain: slot address missing from model");
            drop(lease);
            if let Some(spawn_address) = spawn {
                model.entry(spawn_address).or_insert((value, None));
            }
        }
    }
    let spawned: Vec<_> = bag.lock().expect("reentrant bag lock").drain(..).collect();
    for lease in spawned {
        let (_, spawn) = model
            .remove(lease.address())
            .expect("drain: spawned address missing from model");
        // Spawned endpoints never spawn (one-shot), so `spawn` is None.
        assert!(spawn.is_none(), "drain: spawned endpoint had a spawn");
        drop(lease);
    }
    assert!(space.is_empty() && model.is_empty());
}

/// Release-graph variant of [`fuzz_entry`]: each endpoint may hold the
/// lease of ANOTHER live address (a `LeaseHolder` shape), so releasing a
/// top-level lease cascades through a tree of endpoint drops at ARBITRARY
/// addresses — not the contiguous chains of [`fuzz_entry_cascade`]. A
/// build that attaches a held lease and then collides drops the rejected
/// endpoint, which releases its held lease: the model mirrors both
/// cascade paths.
///
/// Encoding: two bytes per operation. `op = b0 % 4` selects build (0),
/// release (1), resolve (2), or len-check (3); `address = (b0 / 4) % 16`;
/// `b1` selects the held-lease target (`(b1 / 2) % 16`, attached when
/// `b1 % 2 == 1` and the target lease is held by the entry) and salts the
/// endpoint value.
pub fn fuzz_entry_release_graph(data: &[u8]) {
    /// An endpoint that may hold the lease of another address; dropping
    /// it releases that lease (and cascades through the held tree).
    struct Holder {
        #[expect(
            dead_code,
            reason = "the field is exercised through drop (held-lease release), never read"
        )]
        held: Option<Box<bombay_address::Lease<u64, Holder>>>,
    }

    impl Clone for Holder {
        fn clone(&self) -> Self {
            // Resolved snapshots never hold a lease (one-shot).
            Self { held: None }
        }
    }

    let space = bombay_address::AddressSpace::<u64, Holder>::new();
    // Model: address -> (endpoint value, held target). `held` is what the
    // endpoint at that address will release when it is dropped.
    let mut model: std::collections::BTreeMap<u64, (u64, Option<u64>)> = Default::default();
    // Leases the entry holds directly (top-level handles). An address in
    // the model but NOT here is held inside another endpoint.
    let mut handles: std::collections::BTreeMap<u64, bombay_address::Lease<u64, Holder>> =
        Default::default();

    /// Mirror a SUT release: remove `address`, then walk the held chain —
    /// each held lease is the only handle to its registration, so the
    /// drop always releases it.
    fn cascade_release(model: &mut std::collections::BTreeMap<u64, (u64, Option<u64>)>, start: u64) {
        let mut address = Some(start);
        while let Some(a) = address {
            let (_, held) = model
                .remove(&a)
                .expect("cascade: released an address the model does not own");
            address = held;
        }
    }

    for (step, pair) in data.chunks_exact(2).enumerate() {
        let address = u64::from(pair[0] / 4) % 16;
        let held_hint = u64::from(pair[1] / 2) % 16;
        match pair[0] % 4 {
            0 => {
                let want_hold = pair[1] % 2 == 1;
                let attach = want_hold && held_hint != address && handles.contains_key(&held_hint);
                let held = if attach { handles.remove(&held_hint) } else { None };
                match space.claim(address, Holder { held: held.map(Box::new) }) {
                    Ok(lease) => {
                        assert!(
                            model.insert(address, (address, attach.then_some(held_hint))).is_none(),
                            "step {step}: SUT claimed an address the model owns"
                        );
                        handles.insert(address, lease);
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(
                            model.contains_key(&address),
                            "step {step}: SUT rejected an address the model owns"
                        );
                        if attach {
                            // The rejected endpoint (holding the taken
                            // lease) was dropped by `claim`: the held
                            // registration is released with it.
                            cascade_release(&mut model, held_hint);
                        }
                    }
                }
            }
            1 => {
                if let Some(lease) = handles.remove(&address) {
                    drop(lease);
                    cascade_release(&mut model, address);
                }
            }
            2 => {
                // Endpoint values are the address itself; a resolved
                // snapshot is a Clone with `held: None`, so dropping it
                // must never release a registration — if the SUT leaked
                // the held lease into a snapshot, the len check would
                // desync and fail below.
                assert_eq!(
                    space.resolve(&address).map(|_| address),
                    model.get(&address).map(|(value, _)| *value),
                    "step {step}: resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    for (address, lease) in handles {
        drop(lease);
        cascade_release(&mut model, address);
    }
    assert!(space.is_empty() && model.is_empty());
}

/// Colliding-key cascade variant of [`fuzz_entry`]: the chain mechanics
/// of [`fuzz_entry_cascade`] (nested lease chains, mid-build abort
/// cascades) driven through keys that ALL collide in one hash bucket —
/// combining the two attack surfaces. Chain operations must stay exact
/// under constant-hash contention.
///
/// Encoding: two bytes per operation. `op = b0 % 4` selects build (0),
/// cascade (1), resolve (2), or len-check (3); `slot = (b0 / 4) % 4`
/// selects one of four chain bases (`slot * 16`); `b1` gives the build
/// depth (`1 + b1 % 8`) or the resolve offset within the slot's range.
pub fn fuzz_entry_colliding_cascade(data: &[u8]) {
    use std::hash::{Hash, Hasher};

    #[derive(Clone, PartialEq, Eq, Debug)]
    struct Colliding(u64);
    impl Hash for Colliding {
        fn hash<H: Hasher>(&self, state: &mut H) {
            state.write_u64(0);
        }
    }

    /// One chain link: value for identity, plus the next lease down.
    struct Link {
        value: u64,
        #[expect(
            dead_code,
            reason = "the field is exercised through drop (cascade recursion), never read"
        )]
        next: Option<Box<bombay_address::Lease<Colliding, Link>>>,
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

    const SLOT_BASE: u64 = 16;
    const MAX_DEPTH: u64 = 8;

    let space = bombay_address::AddressSpace::<Colliding, Link>::new();
    let mut model: std::collections::BTreeMap<u64, u64> = Default::default();
    type ChainSlot = Option<(bombay_address::Lease<Colliding, Link>, Vec<u64>)>;
    let mut chains: [ChainSlot; 4] = [None, None, None, None];
    for (step, pair) in data.chunks_exact(2).enumerate() {
        let slot = usize::from(pair[0] / 4) % 4;
        let base = (slot as u64) * SLOT_BASE;
        match pair[0] % 4 {
            0 => {
                let depth = 1 + u64::from(pair[1]) % MAX_DEPTH;
                let mut next: Option<Box<bombay_address::Lease<Colliding, Link>>> = None;
                let mut claimed: Vec<u64> = Vec::new();
                let mut aborted = false;
                for offset in (0..depth).rev() {
                    let address = base + offset;
                    match space.claim(
                        Colliding(address),
                        Link { value: address, next: next.take() },
                    ) {
                        Ok(lease) => {
                            assert!(
                                model.insert(address, address).is_none(),
                                "step {step}: SUT claimed an address the model owns"
                            );
                            claimed.push(address);
                            next = Some(Box::new(lease));
                        }
                        Err(bombay_address::AddressInUse(returned)) => {
                            assert_eq!(returned.0, address, "step {step}");
                            assert!(
                                model.contains_key(&address),
                                "step {step}: SUT rejected an address the model owns"
                            );
                            aborted = true;
                            break;
                        }
                    }
                }
                if aborted {
                    for address in claimed {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: partial cascade released an address the model did not own"
                        );
                    }
                } else {
                    let top = *next.expect("depth >= 1");
                    chains[slot] = Some((top, claimed));
                }
            }
            1 => {
                if let Some((top, addresses)) = chains[slot].take() {
                    top.release();
                    for address in addresses {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: cascade released an address the model did not own"
                        );
                    }
                }
            }
            2 => {
                let address = base + u64::from(pair[1]) % MAX_DEPTH;
                assert_eq!(
                    space.resolve(&Colliding(address)).map(|l| l.value),
                    model.get(&address).copied(),
                    "step {step}: resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    for slot in &mut chains {
        if let Some((top, addresses)) = slot.take() {
            top.release();
            for address in addresses {
                model.remove(&address);
            }
        }
    }
    assert!(space.is_empty() && model.is_empty());
}

/// String-key cascade variant of [`fuzz_entry`]: the chain mechanics of
/// [`fuzz_entry_cascade`] driven through short STRING keys on a tiny
/// alphabet with zero-padding variants — exercising the custom chunked
/// hasher's `write` path under nested-lease chain operations (the string
/// fuzz lane has no chains; the cascade lane has no hasher path).
///
/// Encoding: three bytes per operation. `op = b0 % 4` (build, cascade,
/// resolve, len-check); `slot = (b0 / 4) % 4` selects one of four chain
/// bases; `b1` gives the build depth (`1 + b1 % 8`); `b2` selects the
/// string address (length 0..8 over a 4-letter alphabet, optional
/// zero-pad extension).
pub fn fuzz_entry_string_cascade(data: &[u8]) {
    /// One chain link: value for identity, plus the next lease down.
    struct Link {
        value: u64,
        #[expect(
            dead_code,
            reason = "the field is exercised through drop (cascade recursion), never read"
        )]
        next: Option<Box<bombay_address::Lease<String, Link>>>,
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

    const MAX_DEPTH: u64 = 8;

    /// Slot address: a short string over a 4-letter alphabet seeded by
    /// the slot; even offsets with an odd selector get a zero-pad
    /// extension (hasher collision case). Different slots usually build
    /// different strings, but the tiny alphabet still produces cross-slot
    /// collisions the model must track via the abort path.
    fn address_for(slot: usize, offset: u64, selector: u8) -> String {
        let mut address = String::new();
        let length = (usize::from(selector) % 8).max(1);
        for i in 0..length {
            let letter =
                b"ab\0\xff"[usize::from(selector.wrapping_add((slot as u8).wrapping_add(i as u8))) % 4];
            address.push(letter as char);
        }
        if offset.is_multiple_of(2) && selector % 2 == 1 {
            address.push('\0'); // zero-pad extension: hasher collision case
        }
        address
    }

    let space = bombay_address::AddressSpace::<String, Link>::new();
    let mut model: std::collections::BTreeMap<String, u64> = Default::default();
    type ChainSlot = Option<(bombay_address::Lease<String, Link>, Vec<String>)>;
    let mut chains: [ChainSlot; 4] = [None, None, None, None];
    for (step, triple) in data.chunks_exact(3).enumerate() {
        let slot = usize::from(triple[0] / 4) % 4;
        match triple[0] % 4 {
            0 => {
                let depth = 1 + u64::from(triple[1]) % MAX_DEPTH;
                let mut next: Option<Box<bombay_address::Lease<String, Link>>> = None;
                let mut claimed: Vec<String> = Vec::new();
                let mut aborted = false;
                for offset in (0..depth).rev() {
                    let address = address_for(slot, offset, triple[2]);
                    match space.claim(address.clone(), Link { value: offset, next: next.take() }) {
                        Ok(lease) => {
                            assert!(
                                model.insert(address.clone(), offset).is_none(),
                                "step {step}: SUT claimed an address the model owns"
                            );
                            claimed.push(address);
                            next = Some(Box::new(lease));
                        }
                        Err(bombay_address::AddressInUse(returned)) => {
                            assert_eq!(returned, address, "step {step}");
                            assert!(
                                model.contains_key(&address),
                                "step {step}: SUT rejected an address the model owns"
                            );
                            aborted = true;
                            break;
                        }
                    }
                }
                if aborted {
                    for address in claimed {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: partial cascade released an address the model did not own"
                        );
                    }
                } else {
                    let top = *next.expect("depth >= 1");
                    // Replacing a live chain drops its top, cascading the
                    // OLD chain through endpoint drops (non-contiguous
                    // letter-derived string addresses let a rebuild avoid
                    // the old range — unlike the contiguous u64/colliding
                    // variants, where a rebuild always collides and
                    // aborts). Mirror that release before storing.
                    if let Some((old_top, old_addresses)) = chains[slot].take() {
                        drop(old_top);
                        for address in old_addresses {
                            assert!(
                                model.remove(&address).is_some(),
                                "step {step}: replaced chain released an address the model did not own"
                            );
                        }
                    }
                    chains[slot] = Some((top, claimed));
                }
            }
            1 => {
                if let Some((top, addresses)) = chains[slot].take() {
                    top.release();
                    for address in addresses {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: cascade released an address the model did not own"
                        );
                    }
                }
            }
            2 => {
                let address = address_for(slot, u64::from(triple[1]) % MAX_DEPTH, triple[2]);
                assert_eq!(
                    space.resolve(&address).map(|l| l.value),
                    model.get(&address).copied(),
                    "step {step}: resolve({address:?}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    for slot in &mut chains {
        if let Some((top, addresses)) = slot.take() {
            top.release();
            for address in addresses {
                model.remove(&address);
            }
        }
    }
    assert!(space.is_empty() && model.is_empty());
}

/// Isolation × cascade variant of [`fuzz_entry`]: two independent address
/// spaces, each hosting nested lease chains (the `fuzz_entry_cascade`
/// mechanics), with operations routed by a bit — no cross-space bleed
/// under chain mechanics. Each space is model-checked separately at
/// every step.
///
/// Encoding: two bytes per operation. `space = b0 >> 7`; `op = (b0 >> 5)
/// % 4` (build, cascade, resolve, len-check); `slot = (b0 >> 3) % 4`
/// selects one of four chain bases (`slot * 16`) within the space;
/// `b1` gives the build depth (`1 + b1 % 8`) or the resolve offset.
pub fn fuzz_entry_isolation_cascade(data: &[u8]) {
    /// One chain link: value for identity, plus the next lease down.
    struct Link {
        value: u64,
        #[expect(
            dead_code,
            reason = "the field is exercised through drop (cascade recursion), never read"
        )]
        next: Option<Box<bombay_address::Lease<u64, Link>>>,
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

    const SLOT_BASE: u64 = 16;
    const MAX_DEPTH: u64 = 8;

    let spaces = [
        bombay_address::AddressSpace::<u64, Link>::new(),
        bombay_address::AddressSpace::<u64, Link>::new(),
    ];
    let mut models: [std::collections::BTreeMap<u64, u64>; 2] =
        [Default::default(), Default::default()];
    type ChainSlot = Option<(bombay_address::Lease<u64, Link>, Vec<u64>)>;
    let mut chains: [[ChainSlot; 4]; 2] = [[None, None, None, None], [None, None, None, None]];

    for (step, pair) in data.chunks_exact(2).enumerate() {
        let which = usize::from(pair[0] >> 7);
        let slot = usize::from((pair[0] >> 3) % 4);
        let base = (slot as u64) * SLOT_BASE;
        let space = &spaces[which];
        let model = &mut models[which];
        let slot_chains = &mut chains[which];
        match (pair[0] >> 5) % 4 {
            0 => {
                let depth = 1 + u64::from(pair[1]) % MAX_DEPTH;
                let mut next: Option<Box<bombay_address::Lease<u64, Link>>> = None;
                let mut claimed: Vec<u64> = Vec::new();
                let mut aborted = false;
                for offset in (0..depth).rev() {
                    let address = base + offset;
                    match space.claim(address, Link { value: address, next: next.take() }) {
                        Ok(lease) => {
                            assert!(
                                model.insert(address, address).is_none(),
                                "step {step}: space {which} SUT claimed an owned address"
                            );
                            claimed.push(address);
                            next = Some(Box::new(lease));
                        }
                        Err(bombay_address::AddressInUse(returned)) => {
                            assert_eq!(returned, address, "step {step}");
                            assert!(
                                model.contains_key(&address),
                                "step {step}: space {which} SUT rejected an owned address"
                            );
                            aborted = true;
                            break;
                        }
                    }
                }
                if aborted {
                    for address in claimed {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: space {which} partial cascade released an address the model did not own"
                        );
                    }
                } else {
                    let top = *next.expect("depth >= 1");
                    slot_chains[slot] = Some((top, claimed));
                }
            }
            1 => {
                if let Some((top, addresses)) = slot_chains[slot].take() {
                    top.release();
                    for address in addresses {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: space {which} cascade released an address the model did not own"
                        );
                    }
                }
            }
            2 => {
                let address = base + u64::from(pair[1]) % MAX_DEPTH;
                assert_eq!(
                    space.resolve(&address).map(|l| l.value),
                    model.get(&address).copied(),
                    "step {step}: space {which} resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: space {which} len diverged");
            }
        }
        // Cross-check BOTH spaces stay in lockstep with their models.
        for (side, (space, model)) in spaces.iter().zip(models.iter()).enumerate() {
            assert_eq!(
                space.len(),
                model.len(),
                "step {step}: space {side} len diverged"
            );
        }
    }
    for which in 0..2 {
        for slot in &mut chains[which] {
            if let Some((top, addresses)) = slot.take() {
                top.release();
                for address in addresses {
                    models[which].remove(&address);
                }
            }
        }
        assert!(spaces[which].is_empty() && models[which].is_empty());
    }
}

/// Wide-key cascade variant of [`fuzz_entry`]: the chain mechanics of
/// [`fuzz_entry_cascade`] driven through composite `(u64, u64)` keys —
/// exercising the hasher's two-half chunked `write` path under
/// nested-lease chain operations (the wide-key fuzz lane has no chains;
/// the cascade lane has no composite keys).
///
/// Encoding: three bytes per operation. `op = b0 % 4` (build, cascade,
/// resolve, len-check); `slot = (b0 / 4) % 4` selects one of four chain
/// bases; `b1` gives the build depth (`1 + b1 % 8`); `b2` salts the
/// composite key halves.
pub fn fuzz_entry_wide_cascade(data: &[u8]) {
    /// One chain link: value for identity, plus the next lease down.
    struct Link {
        value: u64,
        #[expect(
            dead_code,
            reason = "the field is exercised through drop (cascade recursion), never read"
        )]
        next: Option<Box<bombay_address::Lease<(u64, u64), Link>>>,
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

    const MAX_DEPTH: u64 = 8;

    /// Slot key: `(slot, offset)` salted by `selector`, spanning both
    /// halves of the composite key.
    fn key_for(slot: usize, offset: u64, selector: u8) -> (u64, u64) {
        (
            (slot as u64) * 1_000 + offset + u64::from(selector) % 32,
            (slot as u64) * 1_000_000 + offset * 1_000 + u64::from(selector / 8) % 16,
        )
    }

    let space = bombay_address::AddressSpace::<(u64, u64), Link>::new();
    let mut model: std::collections::BTreeMap<(u64, u64), u64> = Default::default();
    type ChainSlot = Option<(bombay_address::Lease<(u64, u64), Link>, Vec<(u64, u64)>)>;
    let mut chains: [ChainSlot; 4] = [None, None, None, None];
    for (step, triple) in data.chunks_exact(3).enumerate() {
        let slot = usize::from(triple[0] / 4) % 4;
        match triple[0] % 4 {
            0 => {
                let depth = 1 + u64::from(triple[1]) % MAX_DEPTH;
                let mut next: Option<Box<bombay_address::Lease<(u64, u64), Link>>> = None;
                let mut claimed: Vec<(u64, u64)> = Vec::new();
                let mut aborted = false;
                for offset in (0..depth).rev() {
                    let address = key_for(slot, offset, triple[2]);
                    match space.claim(address, Link { value: offset, next: next.take() }) {
                        Ok(lease) => {
                            assert!(
                                model.insert(address, offset).is_none(),
                                "step {step}: SUT claimed an address the model owns"
                            );
                            claimed.push(address);
                            next = Some(Box::new(lease));
                        }
                        Err(bombay_address::AddressInUse(returned)) => {
                            assert_eq!(returned, address, "step {step}");
                            assert!(
                                model.contains_key(&address),
                                "step {step}: SUT rejected an address the model owns"
                            );
                            aborted = true;
                            break;
                        }
                    }
                }
                if aborted {
                    for address in claimed {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: partial cascade released an address the model did not own"
                        );
                    }
                } else {
                    let top = *next.expect("depth >= 1");
                    // Replacing a live chain drops its top, cascading the
                    // OLD chain (a different selector yields different
                    // keys, so a rebuild can avoid the old range).
                    // Mirror that release before storing the new chain
                    // (Segment 23 lesson).
                    if let Some((old_top, old_addresses)) = chains[slot].take() {
                        drop(old_top);
                        for address in old_addresses {
                            assert!(
                                model.remove(&address).is_some(),
                                "step {step}: replaced chain released an address the model did not own"
                            );
                        }
                    }
                    chains[slot] = Some((top, claimed));
                }
            }
            1 => {
                if let Some((top, addresses)) = chains[slot].take() {
                    top.release();
                    for address in addresses {
                        assert!(
                            model.remove(&address).is_some(),
                            "step {step}: cascade released an address the model did not own"
                        );
                    }
                }
            }
            2 => {
                let address = key_for(slot, u64::from(triple[1]) % MAX_DEPTH, triple[2]);
                assert_eq!(
                    space.resolve(&address).map(|l| l.value),
                    model.get(&address).copied(),
                    "step {step}: resolve({address:?}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    for slot in &mut chains {
        if let Some((top, addresses)) = slot.take() {
            top.release();
            for address in addresses {
                model.remove(&address);
            }
        }
    }
    assert!(space.is_empty() && model.is_empty());
}

/// Reentrant-spawn × collision variant of [`fuzz_entry`]: the
/// spawn-on-drop mechanics of [`fuzz_entry_reentrant`] driven through
/// constant-hash keys — every spawn claim, drop-time claim, and release
/// runs through ONE hash bucket (the reentrant lane has no collisions;
/// the colliding lane has no spawns).
///
/// Encoding: two bytes per operation. `op = b0 % 4` (claim, release,
/// resolve, len-check); `address = (b0 / 4) % 16`; `b1` selects the
/// spawn behavior (`b1 % 4 == 0` → none, else spawn at `(b1 / 4) % 16`)
/// and salts the endpoint value.
pub fn fuzz_entry_reentrant_colliding(data: &[u8]) {
    use std::hash::{Hash, Hasher};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, PartialEq, Eq, Debug)]
    struct Colliding(u64);
    impl Hash for Colliding {
        fn hash<H: Hasher>(&self, state: &mut H) {
            state.write_u64(0);
        }
    }

    type Space = bombay_address::AddressSpace<Colliding, Box<Reentrant>>;
    type Bag = Arc<Mutex<Vec<bombay_address::Lease<Colliding, Box<Reentrant>>>>>;

    /// An endpoint whose `Drop` claims a new registration at `spawn`
    /// (one-shot: the spawned endpoint never spawns again).
    struct Reentrant {
        space: Arc<Space>,
        bag: Bag,
        value: u64,
        spawn: Option<u64>,
    }

    impl Clone for Reentrant {
        fn clone(&self) -> Self {
            // Resolved snapshots are inert: a clone never spawns.
            Self {
                space: Arc::clone(&self.space),
                bag: Arc::clone(&self.bag),
                value: self.value,
                spawn: None,
            }
        }
    }

    impl Drop for Reentrant {
        fn drop(&mut self) {
            if let Some(spawn) = self.spawn {
                let spawned = Reentrant {
                    space: Arc::clone(&self.space),
                    bag: Arc::clone(&self.bag),
                    value: self.value,
                    spawn: None,
                };
                if let Ok(lease) = self.space.claim(Colliding(spawn), Box::new(spawned)) {
                    self.bag.lock().expect("reentrant bag lock").push(lease);
                }
            }
        }
    }

    let space: Arc<Space> = Arc::new(bombay_address::AddressSpace::new());
    let bag: Bag = Arc::new(Mutex::new(Vec::new()));

    let mut model: std::collections::BTreeMap<u64, (u64, Option<u64>)> = Default::default();
    let mut leases: [Option<bombay_address::Lease<Colliding, Box<Reentrant>>>; 16] =
        [None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None];
    for (step, pair) in data.chunks_exact(2).enumerate() {
        let address = u64::from(pair[0] / 4) % 16;
        match pair[0] % 4 {
            0 => {
                let value = u64::from(pair[1]) + 1;
                let spawn = if pair[1] % 4 == 0 {
                    None
                } else {
                    Some(u64::from(pair[1] / 4) % 16)
                };
                match space.claim(
                    Colliding(address),
                    Box::new(Reentrant {
                        space: Arc::clone(&space),
                        bag: Arc::clone(&bag),
                        value,
                        spawn,
                    }),
                ) {
                    Ok(lease) => {
                        assert!(
                            model.insert(address, (value, spawn)).is_none(),
                            "step {step}: SUT claimed an address the model owns"
                        );
                        leases[address as usize] = Some(lease);
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned.0, address, "step {step}");
                        assert!(
                            model.contains_key(&address),
                            "step {step}: SUT rejected an address the model owns"
                        );
                        // The rejected endpoint is dropped by `claim`;
                        // its `Drop` may spawn. Mirror it.
                        if let Some(spawn_address) = spawn {
                            model.entry(spawn_address).or_insert((value, None));
                        }
                    }
                }
            }
            1 => {
                if let Some(lease) = leases[address as usize].take() {
                    let (value, spawn) = model
                        .remove(&address)
                        .expect("step {step}: SUT released an address the model does not own");
                    drop(lease);
                    if let Some(spawn_address) = spawn {
                        model.entry(spawn_address).or_insert((value, None));
                    }
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&Colliding(address)).map(|l| l.value),
                    model.get(&address).map(|(value, _)| *value),
                    "step {step}: resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    for (index, slot) in leases.iter_mut().enumerate() {
        if let Some(lease) = slot.take() {
            let (value, spawn) = model
                .remove(&(index as u64))
                .expect("drain: slot address missing from model");
            drop(lease);
            if let Some(spawn_address) = spawn {
                model.entry(spawn_address).or_insert((value, None));
            }
        }
    }
    let spawned: Vec<_> = bag.lock().expect("reentrant bag lock").drain(..).collect();
    for lease in spawned {
        let (_, spawn) = model
            .remove(&lease.address().0)
            .expect("drain: spawned address missing from model");
        assert!(spawn.is_none(), "drain: spawned endpoint had a spawn");
        drop(lease);
    }
    assert!(space.is_empty() && model.is_empty());
}

/// Isolation × reentrant variant of [`fuzz_entry`]: two independent
/// address spaces, each hosting spawn-on-drop endpoints (the
/// `fuzz_entry_reentrant` mechanics), with operations routed by a bit —
/// no cross-space bleed under reentrant spawns. Each space is
/// model-checked separately at every step.
///
/// Encoding: two bytes per operation. `space = b0 >> 7`;
/// `op = (b0 >> 5) % 4` (claim, release, resolve, len-check);
/// `address = (b0 & 0x1f) % 8`; `b1` selects the spawn behavior
/// (`b1 % 4 == 0` → none, else spawn at `(b1 / 4) % 8`) and salts the
/// endpoint value.
pub fn fuzz_entry_isolation_reentrant(data: &[u8]) {
    use std::sync::{Arc, Mutex};

    type Space = bombay_address::AddressSpace<u64, Box<Reentrant>>;
    type Bag = Arc<Mutex<Vec<bombay_address::Lease<u64, Box<Reentrant>>>>>;

    /// An endpoint whose `Drop` claims a new registration at `spawn`
    /// (one-shot: the spawned endpoint never spawns again).
    struct Reentrant {
        space: Arc<Space>,
        bag: Bag,
        value: u64,
        spawn: Option<u64>,
    }

    impl Clone for Reentrant {
        fn clone(&self) -> Self {
            // Resolved snapshots are inert: a clone never spawns.
            Self {
                space: Arc::clone(&self.space),
                bag: Arc::clone(&self.bag),
                value: self.value,
                spawn: None,
            }
        }
    }

    impl Drop for Reentrant {
        fn drop(&mut self) {
            if let Some(spawn) = self.spawn {
                let spawned = Reentrant {
                    space: Arc::clone(&self.space),
                    bag: Arc::clone(&self.bag),
                    value: self.value,
                    spawn: None,
                };
                if let Ok(lease) = self.space.claim(spawn, Box::new(spawned)) {
                    self.bag.lock().expect("reentrant bag lock").push(lease);
                }
            }
        }
    }

    let spaces: [Arc<Space>; 2] = [
        Arc::new(bombay_address::AddressSpace::new()),
        Arc::new(bombay_address::AddressSpace::new()),
    ];
    let bags: [Bag; 2] = [Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new()))];

    let mut models: [std::collections::BTreeMap<u64, (u64, Option<u64>)>; 2] =
        [Default::default(), Default::default()];
    let mut leases: [std::collections::BTreeMap<u64, bombay_address::Lease<u64, Box<Reentrant>>>; 2] =
        [Default::default(), Default::default()];

    for (step, pair) in data.chunks_exact(2).enumerate() {
        let which = usize::from(pair[0] >> 7);
        let address = u64::from(pair[0] & 0x1f) % 8;
        let model = &mut models[which];
        let leases = &mut leases[which];
        match (pair[0] >> 5) % 4 {
            0 => {
                let value = u64::from(pair[1]) + 1;
                let spawn = if pair[1] % 4 == 0 {
                    None
                } else {
                    Some(u64::from(pair[1] / 4) % 8)
                };
                match spaces[which].claim(
                    address,
                    Box::new(Reentrant {
                        space: Arc::clone(&spaces[which]),
                        bag: Arc::clone(&bags[which]),
                        value,
                        spawn,
                    }),
                ) {
                    Ok(lease) => {
                        assert!(
                            model.insert(address, (value, spawn)).is_none(),
                            "step {step}: space {which} SUT claimed an owned address"
                        );
                        leases.insert(address, lease);
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(
                            model.contains_key(&address),
                            "step {step}: space {which} SUT rejected an owned address"
                        );
                        // The rejected endpoint is dropped by `claim`;
                        // its `Drop` may spawn. Mirror it.
                        if let Some(spawn_address) = spawn {
                            model.entry(spawn_address).or_insert((value, None));
                        }
                    }
                }
            }
            1 => {
                if let Some(lease) = leases.remove(&address) {
                    let (value, spawn) = model
                        .remove(&address)
                        .expect("step {step}: space {which} SUT released an address the model does not own");
                    drop(lease);
                    if let Some(spawn_address) = spawn {
                        model.entry(spawn_address).or_insert((value, None));
                    }
                }
            }
            2 => {
                assert_eq!(
                    spaces[which].resolve(&address).map(|l| l.value),
                    model.get(&address).map(|(value, _)| *value),
                    "step {step}: space {which} resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(
                    spaces[which].len(),
                    model.len(),
                    "step {step}: space {which} len diverged"
                );
            }
        }
        // Cross-check BOTH spaces stay in lockstep with their models.
        for (side, (space, model)) in spaces.iter().zip(models.iter()).enumerate() {
            assert_eq!(space.len(), model.len(), "step {step}: space {side} len diverged");
        }
    }
    for which in 0..2 {
        let addresses: Vec<u64> = leases[which].keys().copied().collect();
        for address in addresses {
            if let Some(lease) = leases[which].remove(&address) {
                let (value, spawn) = models[which]
                    .remove(&address)
                    .expect("drain: slot address missing from model");
                drop(lease);
                if let Some(spawn_address) = spawn {
                    models[which].entry(spawn_address).or_insert((value, None));
                }
            }
        }
        let spawned: Vec<_> = bags[which].lock().expect("reentrant bag lock").drain(..).collect();
        for lease in spawned {
            let (_, spawn) = models[which]
                .remove(lease.address())
                .expect("drain: spawned address missing from model");
            assert!(spawn.is_none(), "drain: spawned endpoint had a spawn");
            drop(lease);
        }
        assert!(spaces[which].is_empty() && models[which].is_empty());
    }
}

/// Wide × reentrant variant of [`fuzz_entry`]: 64 addresses with
/// spawn-on-drop endpoints — maximizing simultaneously live
/// registrations under reentrant spawns (the wide lane has no spawns;
/// the reentrant lane has 16 addresses).
///
/// Encoding: one byte per operation. `op = b % 4` (claim, release,
/// resolve, len-check); `address = (b / 4) % 64`; the spawn behavior is
/// derived from the byte (`(b / 4) % 8 == 0` → none, else spawn at
/// `(b / 4) % 64`).
pub fn fuzz_entry_wide_reentrant(data: &[u8]) {
    use std::sync::{Arc, Mutex};

    type Space = bombay_address::AddressSpace<u64, Box<Reentrant>>;
    type Bag = Arc<Mutex<Vec<bombay_address::Lease<u64, Box<Reentrant>>>>>;

    /// An endpoint whose `Drop` claims a new registration at `spawn`
    /// (one-shot: the spawned endpoint never spawns again).
    struct Reentrant {
        space: Arc<Space>,
        bag: Bag,
        value: u64,
        spawn: Option<u64>,
    }

    impl Clone for Reentrant {
        fn clone(&self) -> Self {
            // Resolved snapshots are inert: a clone never spawns.
            Self {
                space: Arc::clone(&self.space),
                bag: Arc::clone(&self.bag),
                value: self.value,
                spawn: None,
            }
        }
    }

    impl Drop for Reentrant {
        fn drop(&mut self) {
            if let Some(spawn) = self.spawn {
                let spawned = Reentrant {
                    space: Arc::clone(&self.space),
                    bag: Arc::clone(&self.bag),
                    value: self.value,
                    spawn: None,
                };
                if let Ok(lease) = self.space.claim(spawn, Box::new(spawned)) {
                    self.bag.lock().expect("reentrant bag lock").push(lease);
                }
            }
        }
    }

    let space: Arc<Space> = Arc::new(bombay_address::AddressSpace::new());
    let bag: Bag = Arc::new(Mutex::new(Vec::new()));

    let mut model: std::collections::BTreeMap<u64, (u64, Option<u64>)> = Default::default();
    let mut leases: std::collections::BTreeMap<u64, bombay_address::Lease<u64, Box<Reentrant>>> =
        Default::default();
    let mut endpoint = 0_u64;
    for (step, &byte) in data.iter().enumerate() {
        let address = u64::from(byte / 4) % 64;
        match byte % 4 {
            0 => {
                endpoint += 1;
                let spawn = if (u64::from(byte / 4) % 8) == 0 {
                    None
                } else {
                    Some(u64::from(byte / 4) % 64)
                };
                match space.claim(
                    address,
                    Box::new(Reentrant {
                        space: Arc::clone(&space),
                        bag: Arc::clone(&bag),
                        value: endpoint,
                        spawn,
                    }),
                ) {
                    Ok(lease) => {
                        assert!(
                            model.insert(address, (endpoint, spawn)).is_none(),
                            "step {step}: SUT claimed an address the model owns"
                        );
                        leases.insert(address, lease);
                    }
                    Err(bombay_address::AddressInUse(returned)) => {
                        assert_eq!(returned, address, "step {step}");
                        assert!(
                            model.contains_key(&address),
                            "step {step}: SUT rejected an address the model owns"
                        );
                        if let Some(spawn_address) = spawn {
                            model.entry(spawn_address).or_insert((endpoint, None));
                        }
                    }
                }
            }
            1 => {
                if let Some(lease) = leases.remove(&address) {
                    let (value, spawn) = model
                        .remove(&address)
                        .expect("step {step}: SUT released an address the model does not own");
                    drop(lease);
                    if let Some(spawn_address) = spawn {
                        model.entry(spawn_address).or_insert((value, None));
                    }
                }
            }
            2 => {
                assert_eq!(
                    space.resolve(&address).map(|l| l.value),
                    model.get(&address).map(|(value, _)| *value),
                    "step {step}: resolve({address}) diverged"
                );
            }
            _ => {
                assert_eq!(space.len(), model.len(), "step {step}: len diverged");
            }
        }
    }
    let addresses: Vec<u64> = leases.keys().copied().collect();
    for address in addresses {
        if let Some(lease) = leases.remove(&address) {
            let (value, spawn) = model
                .remove(&address)
                .expect("drain: slot address missing from model");
            drop(lease);
            if let Some(spawn_address) = spawn {
                model.entry(spawn_address).or_insert((value, None));
            }
        }
    }
    let spawned: Vec<_> = bag.lock().expect("reentrant bag lock").drain(..).collect();
    for lease in spawned {
        let (_, spawn) = model
            .remove(lease.address())
            .expect("drain: spawned address missing from model");
        assert!(spawn.is_none(), "drain: spawned endpoint had a spawn");
        drop(lease);
    }
    assert!(space.is_empty() && model.is_empty());
}

pub mod reservations;
