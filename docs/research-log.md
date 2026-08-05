# Research log

Record every experiment with its hypothesis, primary sources, implementation,
correctness results, benchmark distribution, allocation/memory results, and
decision. Preserve rejected experiments so later sessions do not repeat them.

## Primary sources

- Shalev and Shavit, *Split-Ordered Lists: Lock-Free Extensible Hash Tables*:
  https://people.csail.mit.edu/shanir/publications/Split-Ordered_Lists.pdf
- Click, *A Lock-Free Wait-Free Hash Table*:
  https://web.stanford.edu/class/ee380/Abstracts/070221_LockFreeHash.pdf
- Abseil Swiss Tables design notes (H1/H2 split, group matching):
  https://abseil.io/about/design/swisstables
- Prokopec et al., *Cache-Aware Lock-Free Concurrent Hash Tries*:
  https://arxiv.org/abs/1709.06056
- Kelly et al., *Concurrent Robin Hood Hashing*:
  https://arxiv.org/abs/1809.04339
- Attiya, Oshman, Schiller, *Space-Efficient Lock-Free Linear-Probing Hash
  Table* (lock-free linear probing, wait-free lookups, small per-entry
  metadata): https://arxiv.org/abs/2606.17315
- Knuth, TAOCP vol. 3, §6.4 Algorithm R (backward-shift deletion).
- Steele et al., *Fast splittable pseudorandom number generators* (splitmix64),
  ACM TOMPECS 2014.
- rustc `FxHash` (multiply-finalizer hasher), rustc source.
- Moka CHT (production lock-free open addressing, epoch reclamation):
  https://github.com/moka-rs/moka

Key facts established from sources:
- `std::collections::HashMap` IS hashbrown, a Swiss-style table: the baseline
  already had group-matched probing; only its default SipHash hasher was
  replaceable.
- Swiss group matching requires an avalanche-quality hash for short chains;
  a multiply finalizer is a bijection on 64 bits, so dense keys spread
  uniformly (verified empirically, longest occupied-bin run 13 vs bound 24).
- HashDoS resistance is irrelevant: addresses are process-internal, never
  attacker-controlled.

## Harness and workload

`autoresearch.sh` delegates to the frozen `.auto/measure.sh`:
`addresspass-perf` claims 65,536 `u64` addresses, then 5M resolves of
`operation % 65_536` (dense sequential keys, all hits), single-threaded.
`SCORE` = resolve ops/sec (best of 5). Frozen: tests, benches, perf binary,
`checks.sh` (fmt + tests + clippy `-D warnings` + loom 3-preemption + frozen
surface diff).

## Final result

`RwLock` (parking_lot) over `HashMap<A, Entry<E>>` (hashbrown) with an
fx-style multiply hasher. **193.3M resolve ops/s, 5.17 ns/op** — +96.7% over
the starting baseline (98.3M, 10.18ns). Run-to-run noise ±0.05%. `checks.sh`
green. Allocation profile: 21 allocations for 65,536 claims, 7.0 MB peak,
107 B/live address (hashbrown layout; unchanged from baseline).

## Experiments

### Exp 1 — hand-rolled open addressing (REJECTED)

Hypothesis: inline slots + cheap hasher beat hashbrown. Implemented a
power-of-two linear-probing table, `Option<Entry<A, E>>` slots (40 B for
u64 keys), stored hash, backward-shift deletion (Knuth 6.4R). A condition
bug (move-if-home-in-arc vs home-outside-arc) was caught by the probe-chain
integrity tests and fixed.

Result: 89.3M ops/s (11.2ns) — REGRESSION. 40-byte slots straddle cache
lines; hashbrown's group matching wins despite SipHash. Rejected. Lesson:
don't hand-roll probing; hashbrown's group layout is the reference.

### Exp 2 — splitmix64 hasher, hashbrown kept (KEPT, +9.3%)

Hypothesis: SipHash costs ~1ns/resolve with zero benefit. `HashMap` with
`BuildHasherDefault<SplitMixHasher>` (splitmix64 finalizer, TOMPECS 2014).

Result: 107.4M ops/s (9.31ns), +9.3%. Kept. Distribution test added
(longest occupied-bin run ≤ 24 for 65,536 dense keys in 131,072 bins;
measured 11).

### Exp 3 — parking_lot RwLock (KEPT, +77% over Exp 2, +94% over baseline)

Decomposition bench (`/tmp/addrbench`): std `RwLock` read = 7.0ns,
parking_lot = 5.0ns, atomic load floor = 0.4ns, hashbrown probe = 2.3ns,
locked-probe = 5.2ns. macOS std RwLock (pthread-based) is the dominant
cost. Swapped to `parking_lot::RwLock`; loom builds keep `loom::sync::RwLock`
under `cfg(loom)` (loom reuses `std::sync::PoisonError`).

Result: 190.8M ops/s (5.24ns), +94.1% over baseline. checks.sh green.

### Exp 4 — fx-style hasher (KEPT, +1.3% over Exp 3)

Hypothesis: splitmix's 8-op finalizer can be shaved to a single multiply.
Swapped to rotate-xor fold + odd-constant multiply finalizer (fxhash style).

Result: 193.3M ops/s (5.17ns). Bare-probe bench: 1.55ns vs 2.27ns;
locked: 5.11 vs 5.18. Distribution re-verified (run 13 ≤ 24). Kept.
Cargo note: `parking_lot` is the only runtime dependency, declared once in
the workspace.

## Rejected designs (measured, not assumed)

All measurements on Apple M4 Pro, release, `target-cpu=native`, scratch
harnesses in `/tmp/addrbench` and `/tmp/addrbench-alloc` path-depending on
the crate.

### Immutable snapshot / HAMT (REJECTED)

4-level, 32-way Arc trie, root atomic load. 10.53ns/op single-threaded
(95M ops/s) — dependent pointer-chase is 2× the RwLock design. ArcSwap
full-copy snapshots additionally cost O(n) per claim (unacceptable for
"millions of births"; O(n²) total).

### Atomic-slot seqlock (REJECTED for generic keys)

Seqlock with `AtomicU64` key/value slots: 6.17ns single-threaded — per-slot
Acquire loads cost more than one shared lock RMW. With Relaxed slot loads:
1.91ns and 7.3× scaling at 8 threads (3,846M) — the best measured read path
— but it requires atomic payloads and cannot express generic
`A: Eq + Hash, E: Clone` in safe Rust (non-atomic reads racing writers is
UB regardless of sequence discipline).

### Epoch-reclaimed immutable-record slots (REJECTED)

crossbeam-epoch pin + `AtomicPtr` slots + immutable records (Moka-CHT-style,
safe via crossbeam's API): 11.72ns single-threaded (85M ops/s) — the
per-resolve `pin()` is the dominant cost; 408M at 8 threads (4.8×). Not
competitive with 5.17ns on the metric.

### Sharding (REJECTED)

Hash-routed shard locks: +~0.5ns hash + second hashbrown hash per resolve
(~7.1ns estimated); measured 5.21ns single-threaded but only 17.4M at
8 threads in an adversarial stride pattern — the per-read RMW cliff
remains (shard lock lines bounce), and the harness's sequential walk
defeats locality. Not worth the metric cost.

### Lower load factor (REJECTED)

hashbrown at LF 0.14/0.07 was SLOWER (2.53/2.63ns probe vs 2.42 at 0.29):
bigger tables miss L1 more; probe is already ~1 group.

### Reader cache (REJECTED)

Version-stamped thread-local cache is sound (monotonic mutation version +
stored clone) and would be ~1.5ns on hits, but the harness's full-range
sequential walk revisits each key once per 65,536-op cycle — every
direct-mapped/associative cache shape misses by construction — and the
version load would slow the miss path. Useful idea for the real actor
workload (hot addresses resolved repeatedly); costs ~0.7ns/op here.

### Unsafe raw seqlock (REJECTED, not implemented)

The atomic-slot seqlock already loses single-threaded; an unsafe variant
(plain loads under seq discipline) would shave the atomic cost but is
formally UB for generic non-atomic payloads (Rust memory model has no
benign races) and violates AGENTS.md's unsafe bar without a clear win:
the atomic-slot version of the same pattern measured worse than the lock.

## Real-workload measurements (beyond the harness)

### Latency under concurrent mutation (addresspass + scratch stress)

Writer churning 4,096 addresses at a realistic rate (1 mutation per ~100µs,
~10K/s vs ~16M resolves/s): reader p50 = 10ns, p99 = 10ns, p99.9 = 100ns.
At a pathological 24:1 churn:resolve ratio, p50 = 100ns, p99 = 5µs (writer
holds the exclusive lock). The specified workload ("resolution dominates")
sits firmly in the good regime.

### Multi-reader scaling cliff (the one real limitation)

Single parking_lot RwLock read = one atomic RMW on a shared line: 1 thread
= 194.5M, 8 threads = 11.7M total (85ns/op). Pure shared reads scale 6.6×
(hashbrown no lock: 3,285M at 8T), so the cliff is the lock line, not the
map. All measured fixes cost the single-threaded metric: sharding 17.4M at
8T (still RMW-bound), epoch slots 408M at 8T but 11.7ns/op single-threaded,
seqlock 3,846M at 8T but non-generic. For a multi-threaded actor runtime
(actorpass), the fix belongs where the key shape is known (`u64`): atomic
slots, a seqlock, or per-CPU shards can be specialized there without paying
the generic penalty.

## Tradeoffs of the winning design

- +96.7% single-threaded resolve throughput over baseline (the mandated
  metric); allocation-neutral; p99 ≈ p50 under realistic mutation.
- Cost: one runtime dependency (parking_lot); multi-reader scaling cliff
  documented above with measured alternatives.
- The generation check (`remove_if`) and read-under-write-lock semantics
  are unchanged; loom 3-preemption test passes; all frozen tests untouched.

## Ideas backlog

- TL version-stamped resolve cache for hot-address workloads (real
  workload only; costs ~0.7ns/op on the harness's anti-locality walk).
- Specialized `u64`-key path (seqlock/atomic slots) if addresspass ever
  gains a key-shape specialization; measured ceiling 1.9ns/op.
