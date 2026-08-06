//! Drop-cascade recursion: a chain of N nested leases (each endpoint owns
//! the next lease) is released by dropping the outermost lease. The
//! release path recurses through endpoint drops N levels deep on one
//! thread. This test pins the feasible depth on the default test-thread
//! stack; the ignored companion probes the configured hazard boundary.
#![cfg(not(miri))]

use bombay_address::{AddressSpace, Lease};

/// One chain link: an endpoint holding the next lease down the chain.
struct Link {
    #[expect(
        dead_code,
        reason = "the field is exercised through drop (cascade recursion), never read"
    )]
    next: Option<Box<Lease<u64, Link>>>,
}

impl Clone for Link {
    fn clone(&self) -> Self {
        Self { next: None } // snapshots never carry the chain
    }
}

/// Build a chain of `depth` nested leases; returns the outermost lease.
fn build_chain(space: &AddressSpace<u64, Link>, depth: u64) -> Lease<u64, Link> {
    let mut next: Option<Box<Lease<u64, Link>>> = None;
    // Claim from the innermost address outward so each new endpoint can
    // hold the previous lease.
    for address in (1..=depth).rev() {
        let lease = space.claim(address, Link { next: next.take() }).unwrap();
        next = Some(Box::new(lease));
    }
    *next.unwrap()
}

#[test]
fn cascade_drains_exactly_at_moderate_depth() {
    let space = AddressSpace::new();
    let outer = build_chain(&space, 1_000);
    assert_eq!(space.len(), 1_000);
    outer.release();
    assert!(
        space.is_empty(),
        "1,000-deep cascade must drain exactly without overflow"
    );
}

/// Hazard probe: the recursion depth of a cascade release is the chain
/// length — there is no explicit bound. The correct long-term behavior
/// (for the production owners to choose) is a bounded or iterative
/// cascade; today depth is limited only by stack size. Ignored: whether
/// this overflows depends on the thread's stack, so it is a documented
/// hazard boundary probe, not an active gate. Depth override:
/// `CASCADE_DEPTH=N`.
#[test]
#[ignore = "hazard boundary: cascade release recursion is unbounded; deep chains probe the stack limit"]
fn cascade_depth_boundary_probe() {
    let depth: u64 = std::env::var("CASCADE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000);
    let space = AddressSpace::new();
    let outer = build_chain(&space, depth);
    assert_eq!(space.len(), depth as usize);
    outer.release();
    assert!(space.is_empty());
}
