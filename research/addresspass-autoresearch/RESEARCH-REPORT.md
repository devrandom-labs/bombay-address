# Addresspass adversarial test report

Campaign scope: `.auto/prompt.md` — test-only adversarial verification of the
documented addresspass invariants. Production code is untouched; everything
here lives under `research/addresspass-autoresearch/**` as a detached
workspace crate (`cargo test --manifest-path
research/addresspass-autoresearch/Cargo.toml`).

Attack surfaces probed: atomic collision exclusion, resolve/claim/release
races, stale-generation release, replacement isolation, endpoint lifetime
and clone reentrancy, hash collision behavior, generation exhaustion
boundaries, poison/recovery assumptions, allocation retention,
linearizability.

## Campaign log

### Segment 1 — model-based core, reentrancy, fuzz, loom, Miri

Infrastructure:

- `src/lib.rs` — `ReferenceModel` (independent `BTreeMap` oracle encoding
  the documented semantics; shares no code with `addresspass`),
  `XorShift64Star` (deterministic PRNG for reproducible fuzz), and the two
  fuzzer entry points (`fuzz_entry`, `fuzz_entry_colliding`).
- `fuzz/` — cargo-fuzz crate with two libFuzzer targets calling those
  entry points. The pinned stable toolchain cannot build libFuzzer
  (sanitizer coverage requires nightly), so the campaign executes the same
  entry points through `tests/fuzz_replay.rs`, a fixed-seed mutational
  fuzzer (bit flips, truncation, duplication over a 7-seed corpus;
  xorshift64*). This is deterministic seeded mutation fuzzing, NOT
  coverage-guided fuzzing; stated honestly. Where cargo-fuzz exists:
  `cargo fuzz run addresspass_ops --fuzz-dir research/addresspass-autoresearch/fuzz`.

Results (all native runs on Apple M4 Pro, debug profile, Rust 1.96.0):

- Exhaustive small-state exploration: every history over 2 addresses and
  the alphabet {claim, resolve, release} × {addr0, addr1}, depths 1..=7 —
  335,941 histories replayed against the reference model. **No divergence.**
- Proptest: 512 cases × 2 strategies (uniform histories up to 200 ops over
  16 addresses; hot-address-biased histories), proptest-generated seeds,
  per-step model agreement. **No divergence.** Reproduce:
  `cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test state_machine_proptest`
- Deterministic fuzz replay: 2 campaigns × 22,400+ executions
  (ops seed `0xA55E_0001`, colliding seed `0xC011_1D1E`, 7 hand-written
  seeds each mutated in a deterministic chain). **No divergence.**
- Hash collision correctness: 1,000 constant-hash addresses — claim,
  resolve, partial and full release all exact. **No divergence.**
- Reentrancy: endpoint `Drop` on the failed-claim path, on the release
  path, and endpoint `Clone` on the resolve path all re-enter the address
  space safely (production runs them outside the lock, as documented).
  The released endpoint's re-entrant resolve observes the address already
  gone. **Pass.**
- Deterministic concurrency stress (fixed topologies, `Barrier`
  synchronization for real overlap):
  - 8 threads × 2,000 rounds over 64 shared addresses, per-address owner
    counters: never two live owners. **Pass.**
  - 4 writers churning disjoint 256-address ranges (1,000 claim/release
    cycles each) + 4 readers: no resolve ever returned an endpoint tagged
    for a foreign range. **Pass.**
  - 1 writer × 2,000 generations on one hot address + 6 readers: every
    resolved value inside the claimed range. **Pass.**
  - Cloned spaces across threads share registrations under
    barrier-synchronized interleaving. **Pass.**
- Loom (`LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" cargo test
  --manifest-path research/addresspass-autoresearch/Cargo.toml --test
  loom_model --release`): 3 models — three-claimant exactly-one-winner,
  two readers during release+reclaim, resolve overlapping final release.
  All interleavings within the 3-preemption bound pass. The bound is a
  real limitation: schedules requiring 4+ preemptions are unexplored.
- Miri: see "Miri" section below.

Negative results worth recording:

- **Stale generations are unreachable through the public API.** A lease's
  address cannot be re-registered while that lease lives, and `release`
  consumes the lease, so the generation gate in `remove_if` can never
  observe a mismatch via the API. It is defense-in-depth against future
  internal callers, verified directly only at the (private) table level by
  production's own tests.
- **Generation exhaustion (u64 wraparound) is untestable.** The counter
  starts at 1 and increments only on successful claims; reaching 2^64
  claims is physically infeasible (~585 years at 10^9 claims/s). The
  wraparound hazard (a wrapped generation colliding with a live one) is
  noted as a theoretical boundary, not a finding.

## FINDING-001

**Claim/resolve/release run caller `Hash`/`Eq` code under the table lock;
a re-entrant `Hash` self-deadlocks.**

- Expected: the crate documents that endpoint `Clone`/`Drop` never run
  under the lock precisely so re-entrant caller code cannot deadlock. The
  same re-entrancy safety is expected — but not delivered — for the
  address's `Hash`/`Eq`.
- Actual: `claim` hashes and compares the address (`HashMap::get`, then
  `insert`) while holding the write guard; `resolve`/`release` hash under
  the read/write guard. An address type whose `Hash` or `Eq` calls any
  `AddressSpace` method on the same space deadlocks immediately
  (parking_lot `RwLock` is not re-entrant; the thread already holds the
  lock). Reproduced deterministically: the claim never completes (5 s
  timeout, test fails as designed).
- Severity: low–medium. Requires a pathological-but-legal address type;
  no safe-Rust memory unsafety. It is a liveness hazard contradicting the
  crate's documented re-entrancy design intent, and it is inherited from
  `std::collections::HashMap` semantics (the standard library has the same
  property), so it may be accepted as a documented limitation rather than
  fixed. Recorded for the production owners to decide.
- Affected version: addresspass 0.1.0 (baseline `adc64da`, campaign base
  `d0a4ee2`).
- Reproduce:
  `cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test reentrant_hash_deadlock -- --ignored`
- Regression test: `tests/reentrant_hash_deadlock.rs`,
  `claim_with_reentrant_hash_completes` (ignored; assertion expresses the
  correct behavior — the claim completes). The probe times out and fails
  rather than hanging the suite.
- No fix attempted (production is immutable for this campaign).

## FINDING-002

**A drained address space retains the full peak table allocation forever.**

- Expected: after every lease is released, the space's heap footprint
  returns to within a small bound of its pre-burst size (RAII intuition
  for an "allocation-conscious" address space; the README advertises
  allocation-consciousness and a "millions of births" workload).
- Actual: the backing `HashMap` (hashbrown) never shrinks on removal.
  After claiming and releasing 100,000 `(u64, u64)` registrations, the
  empty space retains **3,276,808 bytes** of table heap
  (~32.8 B/registration at peak capacity), measured with a counting global
  allocator in an isolated test binary. A burst of actor births is
  retained for the life of the space.
- Severity: low (informational). No correctness impact; standard hashbrown
  behavior. Matters for the stated workload only if registration
  population varies by orders of magnitude over the space's lifetime.
- Affected version: addresspass 0.1.0 (baseline `adc64da`, campaign base
  `d0a4ee2`).
- Reproduce:
  `cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test allocation_retention -- --ignored`
- Regression test: `tests/allocation_retention.rs`,
  `full_drain_returns_table_memory` (ignored; assertion expresses the
  correct behavior — retention ≤ 64 KiB after a full drain).
- No fix attempted (production is immutable for this campaign).

## Miri

- Command: `nix develop .#miri --command cargo miri test
  --manifest-path research/addresspass-autoresearch/Cargo.toml`
- Coverage: `tests/miri_ownership.rs` (heap-endpoint model-checked
  history, snapshot-outlives-release, 3-thread churn at small scale) plus
  the non-gated sequential tests (reentrancy, collision correctness at
  population 64, release/reclaim exactness, snapshot independence). The
  exhaustive explorer (335,941 histories), proptest, fuzz replay, and
  stress tests are gated `cfg(not(miri))` or `cfg_attr(miri, ignore)`:
  they are native-speed workloads and would take hours interpreted.
- Result: **all green, exit 0** (nightly 1.99.0-nightly 2026-08-04, Miri
  2026-08-05 build). 3 `miri_ownership` tests (6.2 s interpreted) + 6
  non-gated sequential tests (19.0 s). No undefined behavior, no leaks,
  no data races detected in the exercised paths.

## Interrupted or bounded verification (honest bounds)

- Loom: exhaustive only within `LOOM_MAX_PREEMPTIONS=3`; deeper schedules
  unexplored.
- Fuzzing: deterministic seeded mutation, not coverage-guided (stable
  toolchain constraint). No time-bounded libFuzzer campaign was run.
- Generation exhaustion: untestable (theoretical note above).
- Poison/recovery: production's poison-recovery path exists only under
  `cfg(loom)` (std `RwLock` + `recover`); the native path uses
  parking_lot, which never poisons. A panic-injection loom model is
  planned for a later segment.
