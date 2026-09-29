# Reservation release verification — 2026-09-29

## Workload and invariants

Target: generic, process-internal actor addresses, frequent resolution, and
startup that must exclude duplicate owners before constructing an endpoint.
One address has three states: absent, reserved, or published. Both occupied
states own one nonzero generation; only publication enables resolution.
Acquisition, publication, and removal linearize under the existing table lock.

- Reservation and lease authority are affine; publication moves the same lease.
- Failed acquisition returns the original address and consumes no identity.
- Publication changes the existing entry, with no collision check, key clone,
  reinsertion, or generation allocation. It remains possible after exhaustion.
- Removal checks the generation before deleting either occupied state.
- Endpoint Clone/Drop and address Clone/Drop retain their existing reentrancy
  guarantees. Address Hash/Eq retain the documented non-reentrancy, no-panic,
  stable identity contracts.
- `len` counts occupied addresses; reservations survive rehash/shrink and keep
  the space alive. Resolved snapshots retain endpoint-defined Clone semantics.

## Design review

Reviewed the existing whole-crate implementation, prior benchmark evidence in
`research-log.md`, and these primary sources before choosing the representation:

- Maier, Sanders, Dementiev, [Concurrent Hash Tables: Fast and General?(!)](https://arxiv.org/abs/1601.04017).
  Their work studies scalable concurrent hashing, including the cost of extending
  word-sized tables to more general workloads. It does not establish a faster
  replacement for this crate's generic, reentrant endpoint workload.
- Attiya et al., [History-Independent Concurrent Hash Tables](https://arxiv.org/abs/2503.21016)
  (STOC 2025): a linearizable lock-free Robin Hood design using LL/SC. History
  independence is not required here, and adopting its synchronization scheme
  would introduce substantially different machinery.
- Stable Rust [`HashMap::get_mut`](https://doc.rust-lang.org/std/collections/struct.HashMap.html#method.get_mut)
  and [`Arc`](https://doc.rust-lang.org/std/sync/struct.Arc.html) provide the
  necessary safe mutation and shared endpoint lifetime operations.

The chosen table stores `Option<Arc<E>>`: `None` reserves, `Some` publishes.
A private `Reservation(Lease)` wrapper reuses generation-safe release without
extra ownership flags or duplicated Drop logic. The existing claim path and
reservations share acquisition. No runtime dependency or unsafe production code
was added. Publication allocates only the same endpoint Arc required by a claim.
The existing lock contention limitation remains; this release makes no claim
of universal superiority over concurrent maps.

Alternatives considered: a separate reservation map adds synchronization and
lookup work; an Arc/OnceLock per slot adds allocation and indirection to pending
reservations and changes the resolve path. Neither is needed when mutation is
already serialized by the table lock. An atomic pointer/generation design would
require a new reclamation proof for generic keys and reentrant endpoints.

## Executed verification

Host: Apple M4 Pro, aarch64-darwin. Stable Rust 1.96.0; Nix-pinned nightly
2026-08-05 for Miri and libFuzzer. Baseline: `66cb4e5` (`main`, v0.2.0).

- Workspace unit/integration/doctests cover duplicates, original address
  allocations, ID exhaustion, allocation-free reservation with retained table
  capacity, publication identity, stale reservation/lease drops, unwinding,
  reentrant destructors, key clone counts, non-Clone endpoints, growth/shrink,
  thread migration, and snapshots. Compile-fail tests prohibit cloning and
  double publication.
- Full detached native suite: **88 passed, 8 intentionally ignored historical
  caller-contract/deadlock reproducers**. Includes 1,024 generated histories
  over integer, heap-string, and constant-hash keys, checked at every step
  against an independent three-state oracle. Concurrent stress: 4 threads ×
  20,000 reserve/claim/publish/release rounds over 8 contended addresses.
- Workspace Loom: **9 models passed**, including 5 reservation models.
  Detached Loom: **9 passed, 1 historical ignored model**. Bound: 3 preemptions.
  The detached models were updated for the existing opaque `Resolved` API.
- Miri: **9 public reservation tests + 6 research tests passed**, including
  scaled threaded stress and heap-key model histories. Default Miri checking;
  parking_lot emits an exposed-provenance warning for integer-to-pointer casts,
  so these results do not prove strict provenance in that dependency.
- Coverage-guided libFuzzer: **329,727 executions in 61 seconds**, seed
  `1790711727`, maximum 256 bytes, integer/string/colliding key models, no failure.
  Four deterministic starting histories are retained under `fuzz/seeds/`.
- `nix flake check -L`: build, format, Clippy with warnings denied, nextest,
  doctests, documentation, audit, license checks, and now workspace Loom.
- HTML coverage builds with `nix build .#coverage -L`. Added the missing
  `llvm-tools-preview` component. LLVM reports three mismatched function-data
  records; coverage percentages are not used as a release correctness claim.

## Reproduction

```sh
nix flake check -L
nix build .#coverage -L
nix develop --command cargo test --manifest-path research/address-tests/Cargo.toml --release
nix develop --command env LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS='--cfg loom' CARGO_TARGET_DIR=target/loom-research cargo test --manifest-path research/address-tests/Cargo.toml --test loom_model --release
nix develop .#miri --command env CARGO_TARGET_DIR=target/miri-reservations cargo miri test -p bombay-address --test reservations
nix develop .#miri --command env CARGO_TARGET_DIR=target/miri-reservations cargo miri test --manifest-path research/address-tests/Cargo.toml --test reservations --test miri_ownership
nix develop .#miri --command cargo fuzz run address_reservations research/address-tests/fuzz/seeds/address_reservations --fuzz-dir research/address-tests/fuzz -- -max_total_time=60 -max_len=256 -seed=1790711727
nix develop --command cargo bench -p bombay-address --bench address_space
```

Miri has a separate target directory to prevent reuse of stale interpreter
artifacts. Loom and fuzz campaigns are bounded evidence, not proofs over all
histories. All production code remains safe Rust.

## Benchmark evidence

Criterion, 100 samples. Interleaved baseline/new/baseline binaries, 1 second
warmup and 3 second measurement per resolve population, after concurrent
builds and verification completed. Times below are point estimates in ns/op.

| Workload | Baseline A | Reservations | Baseline B |
| --- | ---: | ---: | ---: |
| Resolve, 1,024 entries | 4.7763 | 4.7331 | 4.7994 |
| Resolve, 65,536 entries | 5.8939 | 5.8482 | 5.8776 |

No resolve regression was observed in this controlled comparison. Earlier
measurements overlapped builds and reported small regressions; those noisy
pilots are retained, not silently discarded. The differences in the final
comparison are too small to justify a general speedup claim.

Lifecycle measurements (100 samples, 3 second warmup, 5 second measurement):
claim/release 23.68 ns, reserve/drop 17.12 ns, reserve/publish/release 24.11 ns,
resolve reserved 3.68 ns, duplicate reservation 3.30 ns. These are single-address
microbenchmarks, not promises about contended production latency.

The counting-allocator regression test verifies no allocations for reserve,
duplicate reserve/claim, resolve-reserved, and reservation drop once capacity
is available; publication performs exactly one endpoint Arc allocation.

[Raw logs](benchmarks/reservations-2026-09-29.txt) retain intervals, pilots, and
sample configuration. To reproduce the A/B/A comparison, build the same
`resolve_hit` benchmark on baseline `66cb4e5` and this branch, preserve both
executables, and run baseline → new → baseline sequentially with
`--bench resolve_hit --warm-up-time 1 --measurement-time 3` in `nix develop`.
