//! Shared support code for the addresspass adversarial test campaign.
//!
//! This crate contains ONLY tests and test infrastructure. It never modifies
//! production code; it exercises `addresspass` as an external dependency.

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

/// A reference model of the documented addresspass semantics, implemented
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
    let space = addresspass::AddressSpace::<u64, u64>::new();
    let mut model = ReferenceModel::new();
    // Per-address live lease slots: (generation, lease).
    let mut leases: [Option<(u64, addresspass::Lease<u64, u64>)>; 16] =
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
                    Err(addresspass::AddressInUse(returned)) => {
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
                assert_eq!(space.resolve(&address), model.resolve(address));
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

    let space = addresspass::AddressSpace::<Colliding, u64>::new();
    let mut model = ReferenceModel::new();
    let mut leases: [Option<(u64, addresspass::Lease<Colliding, u64>)>; 16] =
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
                    Err(addresspass::AddressInUse(returned)) => {
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
                assert_eq!(space.resolve(&Colliding(address)), model.resolve(address));
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
    let space = addresspass::AddressSpace::<String, u64>::new();
    let mut model: std::collections::BTreeMap<String, u64> = Default::default();
    let mut leases: std::collections::BTreeMap<String, addresspass::Lease<String, u64>> =
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
                    Err(addresspass::AddressInUse(returned)) => {
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
                    space.resolve(&address),
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
    let space = addresspass::AddressSpace::<u64, u64>::new();
    let mut model = ReferenceModel::new();
    let mut leases: std::collections::BTreeMap<u64, (u64, addresspass::Lease<u64, u64>)> =
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
                    Err(addresspass::AddressInUse(returned)) => {
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
                assert_eq!(space.resolve(&address), model.resolve(address), "step {step}");
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
