# addresspass

A generic, allocation-conscious concurrent address space for actorpass. It
maps pure address values to typed endpoints while preserving exclusive,
generation-safe ownership.

```rust
use addresspass::AddressSpace;

let space = AddressSpace::new();
let lease = space.claim(7_u64, "endpoint")?;
assert_eq!(space.resolve(&7), Some("endpoint"));
drop(lease);
assert_eq!(space.resolve(&7), None);
# Ok::<(), addresspass::AddressInUse<u64>>(())
```

The initial implementation is intentionally a safe `RwLock<HashMap>` baseline.
It is a semantic reference and measurement baseline, not a claim of optimality.

## Required semantics

- At most one live registration owns an address.
- `resolve` is linearizable with claim and release.
- Releasing an old generation cannot remove a newer generation.
- Endpoint values remain typed; addresspass performs no message erasure.
- No lock is held while caller code uses a resolved endpoint.
- Dropping a lease releases its exact registration.

## Research lanes

```bash
cargo test --workspace
RUSTFLAGS="--cfg loom" cargo test -p addresspass --test loom --release
cargo bench -p addresspass
cargo run -p addresspass-perf --release
nix develop .#miri --command cargo miri test -p addresspass
```

The `/autoresearch` contract lives in `.auto/prompt.md`. Correctness gates and
the score are intentionally separate.
