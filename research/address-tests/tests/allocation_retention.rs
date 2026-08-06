//! FINDING-002 probe: heap retention after a full drain.
//!
//! After every lease is released the address space is logically empty, but
//! the backing hash table keeps its capacity (hashbrown never shrinks on
//! removal): the heap stays allocated. For the "millions of births" actor
//! workload this means a burst of registrations is retained forever.
//!
//! The active assertion expresses the correct expected behavior: draining
//! returns the table's heap to within a small bound of the pre-burst
//! footprint. It currently fails, so the test is ignored pending a
//! production decision.
//!
//! This test binary owns a counting global allocator and runs a single
//! test, so the measurement is isolated from other tests.
#![cfg(not(miri))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn live_bytes() -> isize {
    LIVE.load(Ordering::Relaxed)
}

use bombay_address::AddressSpace;

#[test]
#[ignore = "FINDING-002: the backing table retains its full capacity after every lease is released; a burst is retained for the life of the space"]
fn full_drain_returns_table_memory() {
    // Warm up any one-time allocations (thread-locals, test harness).
    {
        let warm = AddressSpace::<u64, u64>::new();
        let lease = warm.claim(0, 0).unwrap();
        drop(lease);
    }
    let space = AddressSpace::<u64, u64>::new();
    let baseline = live_bytes();

    const BURST: u64 = 100_000;
    let mut leases = Vec::new();
    for address in 0..BURST {
        leases.push(space.claim(address, address).unwrap());
    }
    assert_eq!(space.len(), BURST as usize);
    for lease in leases.drain(..) {
        lease.release();
    }
    drop(leases); // return the Vec buffer before measuring
    assert!(space.is_empty());

    let retained = live_bytes() - baseline;
    // Correct behavior: a drained space returns to within a small bound of
    // its pre-burst footprint (the table itself plus slack), not the burst
    // peak. 100k (u64, generation, Arc<u64>) registrations occupy several
    // MiB; 64 KiB is a generous empty-table bound.
    assert!(
        retained <= 64 * 1024,
        "drained space retains {retained} bytes of table heap (FINDING-002)"
    );
}
