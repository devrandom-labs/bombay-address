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
  335,922 histories replayed against the reference model. **No divergence.**
- Proptest: 512 cases × 2 strategies (uniform histories up to 200 ops over
  16 addresses; hot-address-biased histories), proptest-generated seeds,
  per-step model agreement. **No divergence.** Reproduce:
  `cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test state_machine_proptest`
- Deterministic fuzz replay: 2 campaigns × ~20,000 executions
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

### Segment 3 — endpoint lifecycle, string fuzz, deeper loom

- Endpoint lifecycle accounting (`tests/endpoint_lifecycle.rs`): every
  endpoint handle (claim payload + resolve snapshot clones) carries
  creation/drop counters; after EVERY operation the live-handle count must
  equal the model's registration count, and after the final drain
  created == dropped exactly. Proptest: 256 cases × up to 120 ops over 8
  addresses. **No leak, no double drop.** Deterministic replacement-path
  accounting and boundary-address coexistence (`0`, `1`, `u64::MAX-1`,
  `u64::MAX`, duplicate rejection returning the exact address) pass.
- Third fuzz target `addresspass_strings` (+ `fuzz_entry_strings`):
  3-byte op encoding, addresses over a 4-letter alphabet with zero-padding
  variants. Replay campaign 3: seed `0x5E1E_0003`, ~20,000 executions.
  **No divergence.**
- Loom model added: release racing a fresh claim leaves exactly the new
  owner or empty (4 active models + 1 ignored reproducer).
- Loom bound: `LOOM_MAX_PREEMPTIONS=4` completes all models in 0.6 s;
  `LOOM_MAX_PREEMPTIONS=8` completes all models in 20.6 s, exit 0. The
  campaign gate runs depth 3 (matching the production frozen lane);
  depths 4 and 8 were run manually and are exhaustive within those
  bounds.

### Segment 4 — wider exhaustive exploration, space lifetime

- Exhaustive explorer generalized to `N` addresses: histories over
  {claim, resolve, release} × N addresses, every sequence of depths 1..=d
  replayed from a clean state against the reference model.
  - 2 addresses, depth ≤ 7: **335,922 histories, no divergence.**
  - 3 addresses (two live owners interacting), depth ≤ 6: **597,870
    histories, no divergence.**
- Space lifetime (`tests/space_lifetime.rs`): leases outliving every
  `AddressSpace` handle still release their exact registration; resolved
  snapshots outlive both the space and the registration; `default()`
  parity; `Lease::address` accessor. **All pass.**

### Segment 5 — composable linearizability, wide fuzz

- Linearizability lane extended to scripted multi-address workloads
  (`tests/linearizability.rs`): per-thread scripts are fixed; the
  scheduler picks the interleaving; per-address timed logs are checked
  independently (locality: Herlihy & Wing, TOPLAS 1990). Deterministic
  cases: 4 threads × 5 rounds on one hot address (60 timed ops);
  6 threads × 4 rounds over 3 addresses. Proptest: 64 cases of random
  scripts (2–5 threads × up to 10 ops over 3 addresses). **Every recorded
  history is linearizable.** The checker is re-validated per file against
  fabricated illegal histories.
- Fourth fuzz target `addresspass_wide` (+ `fuzz_entry_wide`): 1-byte op
  encoding over 64 addresses (maximizes live-registration fan-out).
  Replay campaign 4: seed `0x41DE_0004`, ~20,000 executions.
  **No divergence.**
- Observation (recorded, not a finding): `OpenTable::remove_if` performs
  two independent lookups (`get` for the generation check, then
  `remove_entry`). With contract-abiding keys this is exact; a key whose
  `Hash` is stateful (violating the `Hash`/`Eq` consistency contract)
  could make `remove_entry` miss after the `get` hit, and `release`
  ignores the returned `Option` — the lease would report success while
  the entry leaks. Reachable only through caller contract violation, so
  it is documented here rather than filed.

### Segment 6 — snapshot validity, space isolation

- Snapshot validity (`tests/snapshot_validity.rs`): every resolve
  snapshot collected during a random history (256 proptest cases × up to
  100 ops over 6 addresses) is held until the space is fully drained and
  must remain intact; deterministic pinpoint shows a pre-release snapshot
  keeps the OLD value while a post-replacement snapshot holds the NEW
  one. **Pass.**
- Space isolation: deterministic test (same address claimed in two
  spaces; peer release does not interfere) plus a fifth fuzz target
  `addresspass_isolation` routing ops between two spaces, checking BOTH
  spaces against independent models at every resolve step. Replay
  campaign 5: seed `0x1501_A7E5`, ~20,000 executions. **No divergence.**
- Loom model added: two-address/three-thread interleaving has no
  cross-address bleed (6 active models + 1 ignored reproducer).

### Segment 7 — composite keys, lease migration

- Composite keys (`tests/wide_key_proptest.rs`): 256-case proptest over
  `(u64, u64)` tuple addresses against an oracle; order sensitivity of
  tuple parts pinned deterministically; `u128` addresses spanning both
  64-bit halves. **No divergence.**
- Lease migration: a lease claimed on one thread releases exactly on
  another (Miri-friendly test in `space_lifetime.rs`), and a 3-thread
  producer/consumer/resolver handoff over a channel (3,000 rounds over
  32 addresses) never double-owns and drains exactly. **Pass.**

### Segment 8 — deeper bounds, re-entrant contention

- Exhaustive: added 4 addresses depth ≤ 5 (**271,452 histories, no
  divergence**) alongside 2-address depth 7 and 3-address depth 6
  (935,244 histories total per suite run).
- Proptest state machine raised to 1,024 cases per strategy.
- Fuzz replay raised to ~100,000 executions per campaign × 5 campaigns
  (~500,000 total model-checked executions per suite run).
- Re-entrant endpoint drops under 6-thread contention (1,000 rounds
  each): every endpoint's `Drop` calls back into the space; a watchdog
  converts any deadlock regression into a failure. **Pass.**

### Segment 9 — key-identity split (FINDING-005)

Re-review of `claim`'s clone-before-lock path produced the campaign's
strongest finding: the duplicate check and the lease use the caller's
ORIGINAL key while the table stores the CLONE. Two probe key types with
legal interior-mutating `Clone`s demonstrate unresolvable claims, silent
release no-ops with entry leaks, broken exclusive ownership (silent
replacement), and premature endpoint drop under the write guard. See
FINDING-005. An active identity-Clone control test pins harness
validity.

### Segment 10 — breadth adds, deep fuzz soak

- Exhaustive: added 5 addresses depth ≤ 4 (**54,240 histories, no
  divergence**) — 989,484 exhaustive histories per suite run total.
- Loom model added: four claimants over two addresses, exactly one
  winner per address (7 active models + 1 ignored reproducer).
- Fuzz replay gained a `FUZZ_EXECUTIONS` override (deterministic for a
  given count; gate default stays 100,000 per campaign). A deep soak of
  5,000,000 executions per campaign × 5 campaigns (25M total) was run in
  release mode: **all green in 4.4 s — no divergence.** Command:
  `FUZZ_EXECUTIONS=5000000 cargo test --manifest-path
  research/addresspass-autoresearch/Cargo.toml --test fuzz_replay
  --release`

### Segment 11 — nested reentrancy

- Nested release cascade (`tests/nested_reentrancy.rs`): an endpoint
  owning another lease — releasing the outer cascades through endpoint
  drops (3-deep chain), draining exactly with no deadlock. Nested claim:
  an endpoint whose `Drop` claims and releases a new registration.
  Both run under Miri too (no gate). **Pass.**
- Loom model added: nested release racing a resolver of the inner
  address — no torn state, exact drain (8 active models + 1 ignored
  reproducer).

### Segment 12 — drop-cascade depth boundary

- `tests/drop_cascade_depth.rs`: chains of nested leases (each endpoint
  owns the next lease) recurse through `release` → endpoint `Drop` →
  `release` on one thread. 1,000-deep drains exactly (active test).
  Boundary calibration (release profile, default ~2 MiB test-thread
  stack, `CASCADE_DEPTH=N`): **16,000 deep drains; 17,000 deep overflows
  the stack and ABORTS the process** (stack overflow is not unwindable —
  no `catch_unwind` recovery).
- Classification: observation, NOT a finding. The recursion alternates
  addresspass's `release_inner` with the USER endpoint's `Drop` (which
  owns the next lease); the chain shape is constructed through user
  types, and the same unbounded recursion exists for any user-owned
  recursive drop chain (a linked list has the same property). Recorded
  as a hazard boundary: cascade depth is limited only by stack size;
  applications nesting leases deeply must bound the chain or release
  level-by-level. Reproduce:
  `CASCADE_DEPTH=17000 cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test drop_cascade_depth --release -- --ignored`

### Segment 13 — cascade proptest

- `tests/cascade_proptest.rs`: 1–4 independent nested chains (depth 1–63)
  per case, random whole-chain cascades (only the top lease is reachable
  through the public API, so cascades are whole-chain by construction)
  and random resolves, model-checked per step; 256 cases. Pinpoint test:
  top release cascades a whole 3-chain. **No divergence.**

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

### Segment 2 — panic safety, linearizability checking, string addresses, poison lane

New lanes (all native unless noted):

- Panic safety (`tests/panic_safety.rs`): injected `Hash` panics during
  `claim`'s duplicate check and during `claim`'s insert (table pre-seeded:
  hashbrown's `get` on an EMPTY table returns without hashing, so the call
  sequence differs by table occupancy — recorded here as an observed
  implementation fact). Space stays consistent; no ghost registrations.
  Panicking endpoint `Clone` during `resolve` leaves the space usable.
  Re-entrant address `Clone` during `claim` (claims + releases a scratch
  address in the same space) does not deadlock — key `Clone` runs before
  the write guard, as documented. **All pass.** FINDING-003 (below) lives
  in this file.
- Linearizability (`tests/linearizability.rs`): 4 threads × 5 rounds on
  one hot address, every op timed with a global sequence counter; a
  backtracking checker (bitmask DP over per-thread-prefix states,
  real-time precedence) decides whether a legal sequential ordering
  exists. The recorded history **is linearizable**. The checker is
  self-validated: it rejects four fabricated illegal histories
  (double-claim, phantom resolve, stale miss, wrong reject) and accepts
  the legal versions — the test can actually fail.
- String addresses (`tests/string_address_proptest.rs`): proptest state
  machine (256 cases) over short strings on a tiny alphabet plus
  zero-padded variants, against an independent `BTreeMap` oracle.
  **No divergence.** A deterministic test confirms zero-padding collision
  pairs coexist as distinct keys.
- Loom poison lane: the poison-recovery model exposed FINDING-004
  (below); converted to an ignored reproducer.

Hash-quality observation (code inspection, not a defect):
`AddressHasher::write` zero-pads the final short chunk, so `"a"` and
`"a\0\0\0\0\0\0\0"` produce identical 64-bit hashes (verified by tracing
the fold: both yield word `0x61` from the first chunk, identical state
thereafter). Correctness is unaffected (Eq distinguishes the keys; the
coexistence test above passes), and addresses are process-internal, so
this is recorded as a limitation of the hasher's documented
"order-sensitive, cannot collide by permutation" claim — zero-EXTENSION
collisions exist — not as a finding. The hasher is `pub(crate)`, so no
external test can exercise it directly without reimplementation (which
the campaign rules forbid).

## FINDING-003

**`Lease::release` is not panic-atomic: a panicking `Hash`/`Eq` during
removal permanently wedges the address.**

- Expected: release either completes or leaves the registration
  releasable (panic-atomicity). After catching a panic from caller code
  during `release`, the address must be free or the lease must still
  exist to retry.
- Actual: `release_inner` sets `released = true` BEFORE calling
  `remove_if`. If the address's `Hash` (or `Eq`) panics during the
  removal, the unwind consumes the lease — its `Drop` early-returns on
  the `released` flag — while the entry was never removed. The address
  stays claimed forever: `resolve` returns the endpoint, `claim` fails
  with `AddressInUse`, and no handle exists that can ever release it.
- Severity: medium. Requires panicking caller `Hash`/`Eq` (legal safe
  Rust; no unsafe involved). Liveness/availability defect: a permanent
  resource leak of one address per caught panic. Same root-cause class
  as FINDING-001 (caller code runs under the lock); the FIX (production
  owners' call) would be to set `released` only after a successful
  removal.
- Affected version: addresspass 0.1.0 (baseline `adc64da`, campaign base
  `d0a4ee2`).
- Reproduce:
  `cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test panic_safety -- --ignored`
- Regression test: `tests/panic_safety.rs`,
  `release_panic_does_not_wedge_the_address` (ignored; assertion
  expresses the correct behavior — the registration is gone after a
  caught release panic). Fails deterministically: the entry leaks.
- No fix attempted (production is immutable for this campaign).

## FINDING-004

**The loom build's poison recovery is unreachable; any mid-write panic
kills the space permanently (loom builds).**

- Expected: production's `cfg(loom)` path recovers poisoned
  `std::sync::RwLock` guards via `unwrap_or_else(recover)`, so a thread
  panicking while holding the write guard (e.g. caller `Hash` code) does
  not take down the space: later operations recover and observe a
  consistent table.
- Actual: loom 0.7's `RwLock::read()`/`write()` NEVER return
  `Err(PoisonError)` — they return `Ok` unconditionally, and the inner
  guard acquisition does `.expect("loom::RwLock state corrupt")`, which
  panics in-band when the lock is poisoned (loom-0.7.2
  `src/sync/rwlock.rs:52,68,86,102`). `recover` is therefore dead code:
  it can never observe an `Err`. The first operation after a poisoning
  panic dies with `loom::RwLock state corrupt: "Poisoned(..)"` at
  `crates/addresspass/src/lib.rs:36` — in loom builds, one panicking
  caller `Hash` permanently kills the address space.
- Severity: low. The loom configuration is a test-only build; native
  builds use parking_lot, which never poisons (so `recover` is dead code
  natively too). The finding matters because the production research log
  cites the loom lane as covering the std-RwLock semantics, and the
  poison/recovery path is in fact unexercised and unexercisable in its
  current form. Recorded for the production owners; options include
  gating `recover` away honestly or modelling poison with a real
  `std::sync::RwLock` shard inside loom models.
- Affected version: addresspass 0.1.0 (baseline `adc64da`, campaign base
  `d0a4ee2`); loom 0.7.2.
- Reproduce:
  `LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test loom_model --release -- --ignored`
- Regression test: `tests/loom_model.rs`,
  `poisoned_write_lock_recovers_and_stays_consistent` (ignored; assertion
  expresses the correct behavior — post-poison operations succeed and
  the table is consistent). Fails deterministically.
- No fix attempted (production is immutable for this campaign).

## FINDING-005

**`claim` splits the key identity: it duplicate-checks and leases the
ORIGINAL address but stores the CLONE — a legal non-identity `Clone`
breaks resolution, release, exclusive ownership, and endpoint lifetime.**

Root cause: `claim` clones the address before the write guard (by design,
to keep caller `Clone` code out of the lock), then calls
`entries.get(&address)` on the ORIGINAL, `entries.insert(key, ..)` with
the CLONE, and builds the `Lease` from the ORIGINAL. This assumes
`Clone` is identity-preserving — an assumption the
`A: Eq + Hash + Clone` bounds do not license. The probe key types use
interior mutability during `Clone`; they satisfy the
`std::collections::HashMap` key contract at every call site (no key is
ever mutated while stored in the map; `Hash`/`Eq` are consistent for
every value at every instant).

- Expected: (a) a successful claim is immediately resolvable through
  `lease.address()` and exactly releasable; (b) a claim of an address
  `Eq`-equal to a live registration is rejected with `AddressInUse`
  (exclusive ownership); (c) an endpoint is dropped only when its own
  registration is released.
- Actual:
  - **005 (base, desync-down clone):** `claim` returns `Ok` but
    `resolve(lease.address())` is `None` immediately; `lease.release()`
    silently no-ops (the generation gate correctly refuses to remove the
    *other* registration now matching the original's identity); the
    clone-keyed entry leaks for the life of the space.
  - **005b (desync-up clone):** a second claim of an `Eq`-equal address
    is NOT rejected — the duplicate check misses because the table holds
    the desynced clone — and its insert silently REPLACES the live entry.
    Exclusive ownership (the crate's first documented invariant) is
    violated: two live leases span one logical address.
  - **005c (fallout of 005b):** the replacement drops the first endpoint
    inline inside `HashMap::insert` — under the write guard, while its
    lease is still live — contradicting the crate's documented
    "endpoint Drop never runs under the lock" design (with a re-entrant
    endpoint `Drop`, this is FINDING-001's deadlock by a new path).
- Severity: medium–high. Requires an adversarial-but-legal key type;
  realistic actor addresses (Copy integer ids) are unaffected. But the
  broken invariants are the crate's core ones, no unsafe or contract
  violation is involved on the caller side, and the failure is SILENT
  (leak, no panic). The fix is production's call (e.g. insert the
  original and clone for the lease, or document the identity-Clone
  requirement on `claim`).
- Affected version: addresspass 0.1.0 (baseline `adc64da`, campaign base
  `d0a4ee2`).
- Reproduce:
  `cargo test --manifest-path research/addresspass-autoresearch/Cargo.toml --test key_clone_split -- --ignored`
- Regression tests (`tests/key_clone_split.rs`, all ignored, all failing
  deterministically as designed):
  `successful_claim_is_immediately_resolvable_and_releasable` (base),
  `finding_005_leak_shape` (leak shape),
  `duplicate_claim_of_equal_address_is_rejected` (005b),
  `live_endpoint_is_not_dropped_before_its_release` (005c). An ACTIVE
  control (`identity_clone_control_claim_resolve_release_exact`) pins
  that the mechanism under test is the non-identity `Clone`, not the
  harness.
- No fix attempted (production is immutable for this campaign).

## Miri

- Command: `nix develop .#miri --command cargo miri test
  --manifest-path research/addresspass-autoresearch/Cargo.toml`
- Coverage: `tests/miri_ownership.rs` (heap-endpoint model-checked
  history, snapshot-outlives-release, 3-thread churn at small scale) plus
  the non-gated sequential tests (reentrancy, collision correctness at
  population 64, release/reclaim exactness, snapshot independence). The
  exhaustive explorer (335,922 histories), proptest, fuzz replay, and
  stress tests are gated `cfg(not(miri))` or `cfg_attr(miri, ignore)`:
  they are native-speed workloads and would take hours interpreted.
- Result: **all green, exit 0** (nightly 1.99.0-nightly 2026-08-04, Miri
  2026-08-05 build). Final suite: `miri_ownership` (3), `panic_safety`
  (4 active), `sequential_model` (6 active + 3 ignored exhaustive),
  `space_lifetime` (7, incl. cross-thread lease migration). No undefined
  behavior, no leaks, no data races detected in the exercised paths.

## Interrupted or bounded verification (honest bounds)

- Loom: exhaustive within `LOOM_MAX_PREEMPTIONS=8` (20.6 s, exit 0);
  schedules requiring 9+ preemptions unexplored. The poison-recovery
  model is FINDING-004 (ignored).
- Fuzzing: deterministic seeded mutation, not coverage-guided (stable
  toolchain constraint). No time-bounded libFuzzer campaign was run.
- Generation exhaustion: untestable (theoretical note above).
- Poison/recovery: natively untestable (parking_lot never poisons) and
  loom-untestable (FINDING-004). The `recover` path is dead code in both
  build configurations; no executed test can cover it.
- Linearizability checker: histories capped at 63 ops (bitmask DP);
  single hot address; claim/release/resolve only.
