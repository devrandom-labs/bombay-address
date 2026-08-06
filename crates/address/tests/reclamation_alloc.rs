//! Isolated counting-allocator measurement for explicit reclamation.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        #[allow(
            clippy::cast_possible_wrap,
            reason = "Layout sizes stay well below isize::MAX on this test"
        )]
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    #[allow(clippy::cast_possible_wrap, reason = "same as alloc")]
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
fn explicit_shrink_returns_peak_capacity_while_drain_retains_it() {
    // Warm up one-time allocations (test harness, thread-locals).
    {
        let warm = AddressSpace::<u64, u64>::new();
        let lease = warm.claim(0, 0).unwrap();
        drop(lease);
    }
    let space = AddressSpace::<u64, u64>::new();
    let baseline = live_bytes();

    #[allow(
        clippy::cast_possible_truncation,
        reason = "100000 < 2^16; let clippy see it fits"
    )]
    let burst: usize = 100_000;
    let mut leases: Vec<_> = (0..burst)
        .map(|addr| {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "addr from 0..100000 fits u64"
            )]
            let addr = addr as u64;
            space.claim(addr, addr).unwrap()
        })
        .collect();
    assert_eq!(space.len(), burst);
    for lease in leases.drain(..) {
        lease.release();
    }
    drop(leases);
    assert!(space.is_empty());

    // After drain WITHOUT shrink: capacity must be retained (multi-MB).
    let after_drain = live_bytes() - baseline;
    assert!(
        after_drain > 1_024 * 1024,
        "drained space should retain burst capacity, \
         but only {after_drain} bytes; implicit shrink occurred"
    );

    space.shrink_to_fit();
    let after_shrink = live_bytes() - baseline;
    assert!(
        after_shrink <= 1_024 * 1024,
        "after explicit shrink, {after_shrink} bytes retained (bound: 1 MiB)"
    );
}
