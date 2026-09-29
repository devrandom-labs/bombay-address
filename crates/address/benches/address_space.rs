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

fn reservation_lifecycle(c: &mut Criterion) {
    let space = AddressSpace::<u64, u64>::new();
    c.bench_function("claim_release", |b| {
        b.iter(|| drop(space.try_claim(1, 42).unwrap()));
    });
    c.bench_function("reserve_drop", |b| {
        b.iter(|| drop(space.try_reserve(1).unwrap()));
    });
    c.bench_function("reserve_publish_release", |b| {
        b.iter(|| drop(space.try_reserve(1).unwrap().publish(42)));
    });
    let _reserved = space.try_reserve(1).unwrap();
    c.bench_function("resolve_reserved", |b| {
        b.iter(|| std::hint::black_box(space.resolve(&1)));
    });
    c.bench_function("reserve_duplicate", |b| {
        b.iter(|| std::hint::black_box(space.try_reserve(1).err()));
    });
}

criterion_group!(benches, resolve_hit, reservation_lifecycle);
criterion_main!(benches);
