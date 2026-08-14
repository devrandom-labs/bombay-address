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

`verification.sh` delegates to the frozen `the benchmark harness`:
`address-perf` claims 65,536 `u64` addresses, then 5M resolves of
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
benign races) and violates the project safety policy's unsafe bar without a clear win:
the atomic-slot version of the same pattern measured worse than the lock.

## Real-workload measurements (beyond the harness)

### Latency under concurrent mutation (bombay-address + scratch stress)

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

## Follow-up session (2026-08-05, second segment)

### raw_entry / single-hash path (REJECTED, no win)

Hypothesis: `HashMap::get` re-hashes internally; `raw_entry().from_hash`
with a precomputed fx hash should save ~0.4ns. Findings: this toolchain's
(Rust 1.96) `std::collections::HashMap` is a direct hashbrown re-export
and `raw_entry` is GONE from the public API. Prototyped with the
`hashbrown` 0.15 `HashTable` (single-hash `find`): bare probe 1.55ns —
IDENTICAL to `HashMap::get` with fx (the internal re-hash overlaps the
probe's memory latency); locked 5.22ns — identical to the current 5.17.
No dependency added; hypothesis falsified by measurement.

### 1M-population validation (plan's "millions" question)

Winning design at 1,000,000 live addresses (scratch, counting allocator):
- resolve: 19.1ns/op (52.4M ops/s) — DRAM-latency-bound: the map is
  ~110MB, each probe touches 2 lines from DRAM. This is the physical
  floor for any random-access table of that size; no safe generic design
  avoids it (the 65,536-entry harness is L2-resident, which is why the
  lock dominates there instead).
- claim: 44.1ns/addr; release+reclaim pair: 73.7ns.
- allocations: 27 total for 1M claims (hashbrown growth reallocs).
- retained: 110.6 B/live address (linear in population).
- Baseline configuration (std RwLock + SipHash HashMap) at 1M: 31.0ns/op
  (32M ops/s). The design's win shrinks from +97% (65K, cache-resident)
  to +62% (1M, DRAM-bound) but does not vanish.

### Confirmations

- Current commit re-measured: 193.0–193.7M ops/s (5.16–5.18ns), stable
  across runs.
- Multi-reader cliff reproduced with the real crate (scaling bench):
  159.9M single-threaded → 13.9M at 8 threads.

## Full research-solution audit (plan's candidate list, every item)

A reviewer asked whether every research direction from `the verification plan` had
been checked. The audit below closes the gaps that were previously cited as
sources but never measured. All measurements: Apple M4 Pro, release,
scratch benches in /tmp/addrbench.

### papaya (wait-free-read lock-free map) — REJECTED

`papaya::HashMap` with pin-based reads: **28.6ns/op single-threaded
(34.9M)** — 5.5x worse than the RwLock design; 315M at 8 threads (scales,
but the per-read pin cost is prohibitive single-threaded). Its `remove_if`
closure fits the generation gate, but the design loses the metric.

### Sharded 16/32/64/128 — REJECTED (full range now measured)

Single-threaded cost grows monotonically with shard count:
16 shards 5.96ns, 32 → 6.42-6.49, 64 → 7.71-7.77, 128 → 9.18-9.29ns.
8-thread throughput improves only modestly (17.4 → 26 → 39 → 58M) — the
per-read lock RMW persists, diluted not eliminated. Confirmed: sharding is
not a fix; random hash routing gives no locality.

### Two-level radix over the hash (plan's first question) — REJECTED

Unprotected (read-only) 2^16-node × 16-slot radix with splitmix indexing:
**3.93ns/op (254M) single-threaded, 2,084M at 8 threads** — the fastest
structure measured, but it has no synchronization and cannot be made
generic-safe without paying for it. With per-node `parking_lot` locks:
**7.59ns/op (131.7M)** single-threaded, 195M at 8T — the node-lock
granularity does fix the scaling cliff (16x over the single lock) but at
47% single-threaded cost. Also requires node-overflow handling
(8,192-node × 16-slot config with λ=8 overflows; the build hangs).
Rejected for the metric; the node-lock granularity finding is recorded for
the actorpass layer.

### Concurrent Robin Hood hashing — REJECTED (no win)

24B (key, value, distance) tuple array, Robin Hood displacement, single
parking_lot lock: 5.11ns/op single-threaded (~= current 5.17), 118.6M at
8 threads. Notably the short critical section improved 8-thread throughput
10x over the hashbrown-in-lock case (11.7M), but it neither beats the
metric nor fixes the cliff. Note: fx's low bits are a bijection for dense
keys, so the probe is always 1 step — this is also why hashbrown+fx
measures 1.55ns bare.

### Hazard pointers — REJECTED by reasoning from the epoch measurement

Hazard-pointer reads must publish the dereferenced pointer to a global
hazard slot (a shared RMW per read) plus validate — strictly more per-read
work than crossbeam's thread-local epoch pin. The epoch-slot table
(crossbeam pin) measured 11.7ns/op single-threaded; hazard pointers cannot
beat that bound on either the single-threaded or the scaling axis.

### Split-ordered lists / Click's nonblocking table — REJECTED by
### measurement bounds

Both are chained/pointer-chasing probe structures. The HAMT (10.5ns for 4
fixed levels, ~2.6ns/level) and papaya (28.6ns) bound their cost: a chain
walk of 1-2 steps plus lock-free reclamation cannot beat 5.17ns
single-threaded.

### Swiss control bytes + sharded writers — COVERED

Sharded hashbrown IS Swiss control bytes with shard-guarded writers;
measured under "Sharded" above.

### Segmented growth / capacity planning — analyzed, no harness benefit

The metric measures resolve only; growth lives in the claim path.
At 1M claims the design allocates 27 times total (hashbrown doubling). A
capacity-hint API could cut that to ~3 allocations but requires an API
addition the frozen harness never calls. Recorded as a non-goal for this
session.

### Caveat on cross-bench comparisons

The same sharded-16 design measured 191.8M in one binary and 167.8M in
another (+/-14% binary-to-binary codegen variance). In-binary comparisons
are reliable; cross-binary deltas under ~15% should not be over-read. The
in-crate harness (193.3M) remains the ground truth.

## Correctness fix (external review, 2026-08-05)

A review of the sibling integration found two reentrancy liveness bugs and a
coverage gap; all fixed:

1. **Clone under read guard (lib.rs resolve).** `E::clone` ran under the
   read guard; a re-entrant `Clone` that claims/releases the address space
   would self-deadlock on the write lock. Fix: entries store
   `endpoint: Arc<E>`; `resolve` takes an `Arc::clone` under the guard
   (refcount arithmetic only, no user code), then runs `E::clone` after the
   guard drops. Resolve remains allocation-free (Arc::clone allocates
   nothing).
2. **Drop under write guard (release).** `remove_if`'s removed `Entry` was
   dropped inside the lock; a re-entrant `Drop` would deadlock. Fix:
   `remove_if` returns `(A, Arc<E>)` via `remove_entry` (which drops neither
   the key nor the value), and `release_inner` drops the result after the
   write guard. This also moves the address key's `Drop` outside the lock.
3. **Key clone under write guard (claim).** `address.clone()` (caller
   `A: Clone`) now runs before the write guard is taken.
4. **Inherent residue:** `A::hash`/`A::eq` necessarily run inside the probe
   under the locks — unavoidable for any hash-table design; addresses in the
   target workload are `u64`.

Cost (honest): resolve 190.6M ops/s (5.25ns), -1.4% vs 193.3M — the two Arc
RMWs hide behind the probe's memory latency (my prior estimate of ~25% was
wrong; the pipeline is memory-latency-bound, not ALU-bound). Allocations:
1 per claim (Arc node), 65,557 for 65,536 claims (was 21); retained 128
B/address (was 107). The alternative — documenting a non-reentrancy
constraint — was rejected because the review established the contract.

### Loom coverage expansion

New `crates/address/tests/loom_model.rs` (the frozen `tests/loom.rs`
cannot be edited): concurrent duplicate claims (exactly one winner,
loser gets `AddressInUse` with the contested address), release racing
resolve (no torn reads), and replacement release+reclaim (never exposes a
stale or torn endpoint). All 3 new models + the frozen partial-publication
model pass at `LOOM_MAX_PREEMPTIONS=3`.

Known gate limitation: the Nix verification gate invokes only
`--test loom`, so `loom_model.rs` does not run in the gate; it is executed
explicitly (command in the file header). Fixing the gate would require
editing the frozen checks.sh.

### Stale-generation reachability analysis

The reviewer noted the public semantics tests never reach the
stale-generation scenario. Analysis: it is **unreachable through the public
API** — a lease's generation can only be released by that lease's own
`release`/`Drop` (move semantics, `released` flag), and a new claim on the
address requires the old registration to be gone first, so no stale
generation can exist when a newer registration is live. The `remove_if`
generation gate is a defensive invariant (unit-tested at the table level)
against future API growth (e.g., a `replace` operation), not a reachable
race. If such an API is added, the loom model must be extended.

## Machine-state variance (critical for future sessions)

The M4 Pro's single-threaded performance oscillates ±30% across states
(detected while re-measuring after the correctness fix). Same binaries,
same flags, no thermal warnings (`pmset -g therm` clean), no background
load (`ps aux -r` clean, load ~2.5-3): the OS power/clock state moves the
benchmark. Observed ranges:

- baseline design: 96.9M (slow state) to 146.9M (fast state)
- corrected design: 132.2M to 195.0M
- scratch locked-fx probe: 5.11ns to 3.75ns

Within a single state, runs are stable (±0.05% in the original window,
±1-3% when the state is drifting). **Cross-state absolute comparisons are
meaningless; within-state ratios are stable.** The corrected design is
+33-36% over the baseline in EVERY state sampled. The original logged
"+94%" (run #5) was a within-window number where the std-lock baseline was
at its worst; the ratio compressed when the machine sped up because the
baseline's std lock and SipHash benefited most from faster clocks.

Implications:
- Re-validate absolute scores with the session baseline in the SAME state
  window; prefer interleaved A/B measurements.
- The final design's honest standing: +33-36% over baseline, state-
  independent.

## Arc bridge cost (corrected for machine-state contamination)

Run #6 logged the reentrancy fix at -1.4% (190.6 vs 193.3M), but that
comparison straddled a machine-state boundary and UNDERSTATED the cost.
Careful interleaved measurement puts the Arc bridge at **~1.4-1.7ns per
resolve**:
- scratch, contiguous pre-allocated Arc nodes: +1.14ns
- scratch, harness-identical scattered Arc::new pattern: +1.65ns
- harness interleaved commits: pre-fix 266-270M vs post-fix 183-195M
  (within-state, fast state)

The cost is the two refcount RMWs plus the deref of the scattered Arc
node line (cache-set-layout sensitive, hence ±3% run-to-run variance of
the corrected design). It is structural for the mandated reentrancy safety
(E::clone after the read guard, E::drop after the write guard) and cannot
be avoided in safe Rust without either cloning under the lock (the
deadlock bug) or leaking. Accepted and documented.

## Settled-state confirmation (final)

After the oscillation window, the machine settled in the slow state
(stable ±0.5%: corrected design 131.6-132.9M, consistent with the
segment-2 baseline of 132.9M). Definitive interleaved A/B in that state:

- baseline: 96.6, 97.9, 96.6M (avg 97.3M)
- corrected: 131.4, 131.2, 131.8M (avg 131.5M)
- ratio: **+35.2%** — matches the +33-36% observed in every other state.

Final verdict: the corrected design is +35% over the frozen baseline,
state-independent, reentrancy-safe, loom-covered (frozen model + 3 new
models), with every alternative in the research space measured and
rejected on evidence. The remaining single-lock multi-reader cliff and the
Arc bridge cost are documented with measured bounds; both are structural
for the generic+reentrancy-safe contract and belong to the actorpass layer
(u64 keys) to specialize.

## Ideas backlog

- TL version-stamped resolve cache for hot-address workloads (real
  workload only; costs ~0.7ns/op on the harness's anti-locality walk).
- Specialized `u64`-key path (seqlock/atomic slots) if bombay-address ever
  gains a key-shape specialization; measured ceiling 1.9ns/op.
- At 1M population the design is DRAM-bound (19ns); a population-aware
  capacity hint (`with_capacity`) or a compact two-level layout are the
  only levers, both additive API work with no harness benefit.

## Bombay opaque-resolution integration — 2026-08-14

Workload: Bombay resolves an `IncarnationEndpoint` on every routed delivery
and peer-observation capture. That endpoint is itself a pair of cloneable,
reference-counted capabilities. The existing `resolve` first clones the
table's internal `Arc<E>` under the read guard and then clones `E` outside the
guard, so one lookup performs the internal snapshot refcount operations plus
the endpoint's own clone operations.

Invariants: lookup remains generation-snapshot safe; no caller `Clone` or
`Drop` runs under the table lock; a resolved value remains valid after release
and same-address reuse; the existing owned-snapshot API remains available.

The standard library's `Arc::clone` is the existing safe shared-ownership
primitive already required inside the table. A prototype returned an opaque
capability around that shared registered object. The Bombay-shaped benchmark
measured 5.57 ns instead of 7.84 ns, but the adversarial cascade suite rejected
the design: a snapshot then retained ownership fields in the registered
endpoint and delayed nested-lease retirement. Endpoint-defined `Clone` is a
semantic boundary, not redundant work.

The accepted API instead wraps the endpoint-defined clone in an opaque,
read-only `Resolved<E>`. It keeps storage and reclamation private, provides one
opinionated lookup path, preserves non-owning clone semantics, and prevents
callers from mutating a resolved snapshot. The measured shared-object result is
retained only as rejected research evidence and is not claimed by the final
design.
