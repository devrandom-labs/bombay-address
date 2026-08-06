use bombay_address::AddressSpace;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn resolve_hit(c: &mut Criterion) {
    let mut group = c.benchmark_group("resolve_hit");
    for size in [1_024_u64, 65_536] {
        let space = AddressSpace::new();
        let leases = (0..size)
            .map(|address| space.claim(address, address).unwrap())
            .collect::<Vec<_>>();
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, size| {
            let mut address = 0;
            b.iter(|| {
                address = (address + 1) % size;
                std::hint::black_box(space.resolve(&address))
            });
        });
        drop(leases);
    }
    group.finish();
}

criterion_group!(benches, resolve_hit);
criterion_main!(benches);
