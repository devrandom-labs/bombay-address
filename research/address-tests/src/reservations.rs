//! Independent three-state oracle shared by property tests and libFuzzer.
use bombay_address::{AddressSpace, ClaimError, Lease, Reservation};
use std::{
    collections::{BTreeMap, HashSet},
    hash::Hash,
};

/// Check a byte history over eight logical addresses, including collisions.
pub fn check_history<A: Eq + Hash + Clone + std::fmt::Debug>(data: &[u8], key: impl Fn(u8) -> A) {
    let space = AddressSpace::new();
    let mut expected = BTreeMap::<u8, Option<u64>>::new();
    let mut reserved = BTreeMap::<u8, Reservation<A, u64>>::new();
    let mut leases = BTreeMap::<u8, Lease<A, u64>>::new();
    let mut identities = HashSet::new();
    let mut snapshots = Vec::new();
    for (step, byte) in data.iter().copied().enumerate() {
        let address = (byte / 8) % 8;
        let endpoint = step as u64;
        match byte % 8 {
            0 | 1 => {
                let result = space.try_reserve(key(address));
                if expected.contains_key(&address) {
                    assert!(
                        matches!(result, Err(ClaimError::AddressInUse(a)) if a == key(address))
                    );
                } else {
                    let r = result.ok().expect("free address must be reservable");
                    assert!(identities.insert(r.registration_id()));
                    reserved.insert(address, r);
                    expected.insert(address, None);
                }
            }
            2 => {
                if let Some(r) = reserved.remove(&address) {
                    let identity = r.registration_id();
                    let lease = r.publish(endpoint);
                    assert_eq!(lease.registration_id(), identity);
                    leases.insert(address, lease);
                    expected.insert(address, Some(endpoint));
                }
            }
            3 => {
                let result = space.try_claim(key(address), endpoint);
                if expected.contains_key(&address) {
                    assert!(
                        matches!(result, Err(ClaimError::AddressInUse(a)) if a == key(address))
                    );
                } else {
                    let lease = result.ok().expect("free address must be claimable");
                    assert!(identities.insert(lease.registration_id()));
                    leases.insert(address, lease);
                    expected.insert(address, Some(endpoint));
                }
            }
            4 | 5 => {
                drop(reserved.remove(&address));
                if let Some(lease) = leases.remove(&address) {
                    if byte % 8 == 4 {
                        drop(lease);
                    } else {
                        lease.release();
                    }
                }
                expected.remove(&address);
            }
            6 => {
                let actual = space.resolve(&key(address));
                assert_eq!(
                    actual.as_deref().copied(),
                    expected.get(&address).copied().flatten()
                );
                if let Some(value) = actual {
                    snapshots.push((*value, value));
                }
            }
            7 => space.shrink_to_fit(),
            _ => unreachable!(),
        }
        assert_eq!(space.len(), expected.len());
        assert_eq!(space.is_empty(), expected.is_empty());
        for a in 0..8 {
            assert_eq!(
                space.resolve(&key(a)).as_deref().copied(),
                expected.get(&a).copied().flatten()
            );
        }
        for (value, snapshot) in &snapshots {
            assert_eq!(snapshot.as_ref(), value);
        }
    }
    drop((reserved, leases));
    assert!(space.is_empty());
}

/// Different addresses with a deliberately identical hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colliding(pub u8);
impl Hash for Colliding {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u8(0);
    }
}

/// libFuzzer and deterministic replay entry point.
pub fn fuzz_entry(data: &[u8]) {
    check_history(data, u64::from);
    check_history(data, Colliding);
    check_history(data, |a| format!("address-{a}"));
}
