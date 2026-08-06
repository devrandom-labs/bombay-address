use std::hint::black_box;
use std::time::Instant;

use bombay_address::AddressSpace;

#[allow(
    clippy::cast_precision_loss,
    reason = "aggregate benchmark metrics intentionally use floating point"
)]
fn metrics(elapsed: std::time::Duration) -> (f64, f64) {
    (
        OPERATIONS as f64 / elapsed.as_secs_f64(),
        elapsed.as_nanos() as f64 / OPERATIONS as f64,
    )
}

const ENTRIES: u64 = 65_536;
const OPERATIONS: u64 = 5_000_000;

fn main() {
    let space = AddressSpace::new();
    let leases = (0..ENTRIES)
        .map(|address| space.claim(address, address).expect("unique address"))
        .collect::<Vec<_>>();

    let started = Instant::now();
    for operation in 0..OPERATIONS {
        black_box(space.resolve(black_box(&(operation % ENTRIES))));
    }
    let elapsed = started.elapsed();
    let (throughput, latency) = metrics(elapsed);
    println!("SCORE={throughput:.3}");
    println!("THROUGHPUT_OPS={throughput:.3}");
    println!("RESOLVE_NS={latency:.3}");
    black_box(leases);
}
