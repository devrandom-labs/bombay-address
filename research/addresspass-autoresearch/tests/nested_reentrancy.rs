//! Nested reentrancy: an endpoint's `Drop` drops ANOTHER lease (nested
//! release) or CLAIMS a new registration. Production drops removed
//! endpoints after the write guard is released; these tests pin that the
//! guarantee holds one nesting level deep — the recursion through
//! endpoint drops must not deadlock and must stay exact.
//!
//! Small and thread-light: also runs under Miri.

use std::sync::Arc;

use addresspass::{AddressSpace, Lease};

/// An endpoint that owns a lease of a DIFFERENT address in the same
/// space; dropping the endpoint releases that lease.
struct LeaseHolder {
    #[expect(
        dead_code,
        reason = "the field is exercised through drop (nested release), never read"
    )]
    inner: Option<Lease<u64, Box<LeaseHolder>>>,
}

impl Clone for LeaseHolder {
    fn clone(&self) -> Self {
        Self { inner: None } // clones never carry a lease (one-shot)
    }
}

#[test]
fn release_drops_endpoint_which_releases_another_lease() {
    let space = Arc::new(AddressSpace::<u64, Box<LeaseHolder>>::new());
    // Chain: lease for address 1 holds the lease for address 2, which
    // holds the lease for address 3.
    let third = space.claim(3_u64, Box::new(LeaseHolder { inner: None })).unwrap();
    let second = space
        .claim(
            2_u64,
            Box::new(LeaseHolder {
                inner: Some(third),
            }),
        )
        .unwrap();
    let first = space
        .claim(
            1_u64,
            Box::new(LeaseHolder {
                inner: Some(second),
            }),
        )
        .unwrap();
    assert_eq!(space.len(), 3);

    // Releasing address 1 must cascade: drop endpoint 1 → releases 2 →
    // drop endpoint 2 → releases 3. No deadlock, exact drain.
    first.release();
    assert!(space.resolve(&1).is_none());
    assert!(space.resolve(&2).is_none());
    assert!(space.resolve(&3).is_none());
    assert!(space.is_empty());
}

/// An endpoint whose `Drop` CLAIMS a new registration in the same space.
struct ClaimOnDrop {
    space: Arc<AddressSpace<u64, Box<ClaimOnDrop>>>,
    spawn_address: u64,
}

impl Clone for ClaimOnDrop {
    fn clone(&self) -> Self {
        Self {
            space: Arc::clone(&self.space),
            spawn_address: self.spawn_address,
        }
    }
}

impl Drop for ClaimOnDrop {
    fn drop(&mut self) {
        // Runs after the write guard is released (by design); the nested
        // claim must succeed and must not deadlock.
        if self.spawn_address > 0 {
            let spawned = ClaimOnDrop {
                space: Arc::clone(&self.space),
                spawn_address: 0, // one-shot: spawned endpoint never claims
            };
            let lease = self.space.claim(self.spawn_address, Box::new(spawned));
            assert!(lease.is_ok(), "nested claim during endpoint drop failed");
            // Drop the spawned lease immediately so the final drain is
            // exact (its endpoint never claims: spawn_address == 0).
            drop(lease);
        }
    }
}

#[test]
fn release_drops_endpoint_which_claims_and_releases() {
    let space = Arc::new(AddressSpace::<u64, Box<ClaimOnDrop>>::new());
    let endpoint = ClaimOnDrop {
        space: Arc::clone(&space),
        spawn_address: 99,
    };
    let lease = space.claim(1_u64, Box::new(endpoint)).unwrap();
    assert_eq!(space.len(), 1);
    lease.release();
    // The nested claim+release happened inside the drop; final state is
    // empty and consistent.
    assert!(space.is_empty());
    let probe = space.claim(99_u64, Box::new(ClaimOnDrop {
        space: Arc::clone(&space),
        spawn_address: 0,
    }));
    assert!(probe.is_ok(), "address 99 must be free after the nested cycle");
    assert_eq!(space.len(), 1);
    probe.unwrap().release();
    assert!(space.is_empty());
}
