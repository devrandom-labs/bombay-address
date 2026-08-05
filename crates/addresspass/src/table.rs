//! Hash function and table wrapper for the address space.
//!
//! [`std::collections::HashMap`] is hashbrown: a Swiss-style open-addressed
//! table with group matching (abseil design notes), which keeps probe costs
//! low even at high load. The baseline measurement showed that with the
//! default `SipHash` hasher the resolve path spent several nanoseconds in
//! hashing alone. Addresses are process-internal and never attacker
//! controlled, so `SipHash`'s `HashDoS` resistance buys nothing here; the
//! resolve hot path gets the multiply-finalizer scheme used by rustc's
//! `FxHash` (a bijective multiply by an odd constant, which also spreads
//! dense sequential keys onto hashbrown's group index bits).

#[cfg(loom)]
use loom::sync::Arc;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hash, Hasher};
#[cfg(not(loom))]
use std::sync::Arc;

/// Cheap hasher for process-internal addresses, in the style of rustc's
/// `FxHash`: rotate-xor folds on `write`, one odd-constant multiply on
/// `finish`. The multiply is a bijection on 64 bits, so dense sequential
/// keys map onto hashbrown's index bits without clustering (verified by
/// the `splitmix_distributes_dense_keys_evenly` test, which now exercises
/// this hasher).
#[derive(Default)]
pub(crate) struct AddressHasher(u64);

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for AddressHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0.wrapping_mul(FX_SEED)
    }

    /// Fold byte spans into the state, order-sensitively, so multi-part
    /// keys (e.g. `&str` plus its `0xff` terminator) cannot collide by
    /// permutation of their parts.
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0_u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            let word = u64::from_le_bytes(word);
            self.0 = self.0.rotate_left(5) ^ word;
        }
    }

    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.0 = self.0.rotate_left(5) ^ n;
    }

    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.0 = self.0.rotate_left(5) ^ u64::from(n);
    }
}

/// Derive the 64-bit probe hash for an address.
#[cfg(test)]
#[inline]
pub(crate) fn hash_key<A: Hash>(address: &A) -> u64 {
    let mut hasher = AddressHasher::default();
    address.hash(&mut hasher);
    hasher.finish()
}

/// One live registration: the registration generation and the typed
/// endpoint. The address is the hash map key.
///
/// The endpoint is stored behind an [`Arc`] so that [`resolve`](crate::AddressSpace::resolve)
/// can take a reference-counted handle under the read guard and run the
/// endpoint's `Clone` implementation *after* the guard is dropped. Running
/// caller endpoint code (Clone or Drop) under the lock would deadlock if
/// that code re-entered the address space.
pub(crate) struct Entry<E> {
    pub(crate) generation: u64,
    pub(crate) endpoint: Arc<E>,
}

/// The registration table: a Swiss-style open-addressed map keyed by
/// address, hashed with splitmix64.
pub(crate) struct OpenTable<A, E> {
    entries: HashMap<A, Entry<E>, BuildHasherDefault<AddressHasher>>,
}

impl<A, E> OpenTable<A, E> {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::default(),
        }
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<A: Eq + Hash, E> OpenTable<A, E> {
    /// Borrow the registration for `address`, if any.
    #[must_use]
    pub(crate) fn get(&self, address: &A) -> Option<&Entry<E>> {
        self.entries.get(address)
    }

    /// Insert a registration. The caller must have verified `address` is
    /// not already present.
    pub(crate) fn insert(&mut self, address: A, generation: u64, endpoint: E) {
        self.entries.insert(
            address,
            Entry {
                generation,
                endpoint: Arc::new(endpoint),
            },
        );
    }

    /// Remove the registration for `address` only when its generation
    /// matches `generation`. This is the generation safety gate: a stale
    /// lease can never remove a newer registration for the same address.
    ///
    /// Returns the removed address and endpoint handle so the caller can
    /// drop them outside the write guard: the endpoint's `Drop` (through
    /// the [`Arc`]) and the address's `Drop` must not run while the lock
    /// is held, or re-entrant destructors would deadlock.
    #[must_use]
    pub(crate) fn remove_if(&mut self, address: &A, generation: u64) -> Option<(A, Arc<E>)> {
        if self
            .entries
            .get(address)
            .is_some_and(|entry| entry.generation != generation)
        {
            return None;
        }
        // `remove_entry` returns the key and value instead of dropping them.
        self.entries
            .remove_entry(address)
            .map(|(key, entry)| (key, entry.endpoint))
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenTable, hash_key};

    #[test]
    fn insert_get_roundtrip() {
        let mut table = OpenTable::new();
        table.insert(7_u64, 1, 10_u64);
        assert_eq!(table.len(), 1);
        let entry = table.get(&7).expect("present");
        assert_eq!(entry.generation, 1);
        assert_eq!(entry.endpoint.as_ref(), &10);
        assert!(table.get(&8).is_none());
    }

    #[test]
    fn scattered_removal_preserves_all_remaining_lookups() {
        let mut table = OpenTable::new();
        for key in 0..4_000_u64 {
            table.insert(key, key + 1, key.wrapping_mul(3));
        }
        for key in 0..4_000_u64 {
            if key % 10 != 0 {
                assert!(table.remove_if(&key, key + 1).is_some());
            }
        }
        for key in 0..4_000_u64 {
            if key % 10 == 0 {
                let entry = table.get(&key).expect("survivor must be found");
                assert_eq!(entry.generation, key + 1);
                assert_eq!(entry.endpoint.as_ref(), &key.wrapping_mul(3));
            } else {
                assert!(table.get(&key).is_none(), "deleted key {key} still present");
            }
        }
        assert_eq!(table.len(), 400);
    }

    #[test]
    fn stale_generation_never_removes_newer_registration() {
        let mut table = OpenTable::new();
        table.insert(1_u64, 1, "first");
        assert!(table.remove_if(&1, 1_000).is_none());
        assert_eq!(table.get(&1).expect("kept").endpoint.as_ref(), &"first");
        assert!(table.remove_if(&1, 1).is_some());
        assert!(table.get(&1).is_none());
    }

    #[test]
    fn growth_rehashes_without_losing_entries() {
        let mut table = OpenTable::new();
        for key in 0..10_000_u64 {
            table.insert(key, key + 1, key * 7);
        }
        assert_eq!(table.len(), 10_000);
        for key in 0..10_000_u64 {
            let entry = table.get(&key).expect("present after growth");
            assert_eq!(entry.generation, key + 1);
            assert_eq!(entry.endpoint.as_ref(), &(key * 7));
        }
        for key in 0..10_000_u64 {
            if key % 3 == 0 {
                assert!(table.remove_if(&key, key + 1).is_some());
            }
        }
        for key in 0..10_000_u64 {
            if key % 3 == 0 {
                assert!(table.get(&key).is_none());
            } else {
                assert_eq!(
                    table.get(&key).expect("survivor").endpoint.as_ref(),
                    &(key * 7)
                );
            }
        }
    }

    #[test]
    fn splitmix_distributes_dense_keys_evenly() {
        // Dense sequential keys must not cluster: with 65_536 keys and a
        // 131_072-slot table, the longest run of consecutive occupied bins
        // must stay small, otherwise probe chains would grow with
        // population. Expected longest run for 65_536 balls in 131_072 bins
        // is ~log2(131_072) ≈ 17; 24 is a generous ceiling.
        let mut buckets = vec![0_u64; 131_072];
        #[allow(
            clippy::cast_possible_truncation,
            reason = "distribution test tolerates 32-bit pointer truncation"
        )]
        for key in 0..65_536_u64 {
            buckets[hash_key(&key) as usize & 131_071] += 1;
        }
        let longest = buckets.iter().map(|&b| u64::from(b > 0)).fold(
            (0_u64, 0_u64),
            |(best, current), run| {
                if run == 1 {
                    (best.max(current + 1), current + 1)
                } else {
                    (best, 0)
                }
            },
        );
        assert!(longest.0 <= 24, "worst primary-slot run {}", longest.0);
    }
}
