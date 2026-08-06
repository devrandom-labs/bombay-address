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

/// An endpoint whose `Drop` CLAIMS a new registration in the same space
/// and PARKS the spawned lease (the spawned registration persists). The
/// claim may fail if the address is taken — the drop must tolerate that.
type ParkBag = Arc<std::sync::Mutex<Vec<Lease<u64, Box<ParkOnDrop>>>>>;
struct ParkOnDrop {
    space: Arc<AddressSpace<u64, Box<ParkOnDrop>>>,
    park: ParkBag,
    spawn_address: Option<u64>,
}

impl Clone for ParkOnDrop {
    fn clone(&self) -> Self {
        Self {
            space: Arc::clone(&self.space),
            park: Arc::clone(&self.park),
            spawn_address: None, // resolved snapshots never spawn
        }
    }
}

impl Drop for ParkOnDrop {
    fn drop(&mut self) {
        if let Some(spawn) = self.spawn_address {
            let spawned = ParkOnDrop {
                space: Arc::clone(&self.space),
                park: Arc::clone(&self.park),
                spawn_address: None,
            };
            if let Ok(lease) = self.space.claim(spawn, Box::new(spawned)) {
                self.park.lock().unwrap().push(lease);
            }
        }
    }
}

/// A FAILED claim still runs the rejected endpoint's `Drop` (production
/// drops it outside the write guard); if that `Drop` claims a new
/// registration, the spawn persists and must be resolvable.
#[test]
fn failed_claim_runs_endpoint_drop_which_parks_spawn() {
    let space = Arc::new(AddressSpace::<u64, Box<ParkOnDrop>>::new());
    let park = Arc::new(std::sync::Mutex::new(Vec::new()));

    let first = space
        .claim(
            1_u64,
            Box::new(ParkOnDrop {
                space: Arc::clone(&space),
                park: Arc::clone(&park),
                spawn_address: Some(99),
            }),
        )
        .unwrap();
    assert_eq!(space.len(), 1);

    // Second claim of address 1 is rejected; the rejected endpoint's Drop
    // claims 99 and parks the lease — so 99 becomes live.
    let rejected = space.claim(
        1_u64,
        Box::new(ParkOnDrop {
            space: Arc::clone(&space),
            park: Arc::clone(&park),
            spawn_address: Some(99),
        }),
    );
    assert!(rejected.is_err(), "duplicate claim must be rejected");
    assert_eq!(space.len(), 2, "rejected endpoint's drop must spawn 99");
    assert!(space.resolve(&99).is_some(), "spawned 99 must be live");

    // Drain: release 1 (its endpoint's drop tries to spawn 99 again — now
    // taken, tolerated), then the parked 99.
    first.release();
    let parked: Vec<_> = park.lock().unwrap().drain(..).collect();
    assert_eq!(parked.len(), 1);
    for lease in parked {
        lease.release();
    }
    assert!(space.is_empty());
}
