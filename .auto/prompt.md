# Autoresearch: discover the best address space for actorpass

## Mission

Find the fastest, most memory-efficient implementation that satisfies the
frozen addresspass semantics. This is an open research program, not permission
to assume that lock-free is automatically superior.

Before proposing an algorithm, search the web and read primary sources. Start
with concurrent hash-table papers and production designs including split-
ordered lists, Cliff Click's nonblocking table, cache-aware Ctries, concurrent
Robin Hood hashing, Swiss-table metadata/probing, sharded maps, RCU/snapshot
maps, epoch reclamation, hazard pointers, and address-derived slot tables.
Investigate newer relevant work too. Record links and applicability in
`docs/research-log.md`; clearly distinguish sourced claims from inference.

## Actual workload

- Resolution dominates and must have excellent p50 and p99 latency.
- Birth and termination mutate the table concurrently with resolution.
- Address collisions must be rejected atomically.
- An old registration must never remove a newer registration.
- Resolved typed endpoints are cheap to clone and used after lookup returns.
- Actor populations range from tens to millions.
- Allocation count and retained bytes per live address are first-class metrics.
- No actor, mailbox, scheduler, message, or supervision knowledge may enter.

## Method

1. State one falsifiable hypothesis.
2. Add or select the workload that tests it.
3. Run the baseline repeatedly and record noise.
4. Make one coherent implementation change.
5. Run `.auto/checks.sh`; discard any semantic regression.
6. Run `.auto/measure.sh` repeatedly and compare distributions, not one lucky run.
7. Record outcome, allocations, memory, p50/p99, throughput, and tradeoffs.
8. Keep only changes with reproducible value; commit each accepted experiment.

Do not edit frozen tests, the perf harness, checks, or metric definitions. Do
not introduce `unsafe` merely to explore. Before any unsafe experiment, write
the safety invariant and memory-reclamation proof obligation, then add Loom and
Miri coverage that would fail when the invariant is inverted.

## Candidate questions

- Does actorpass's address shape permit direct indexing or two-level radix
  indexing that beats generic hashing?
- Does sharding beat a single read lock at realistic mutation rates?
- Can immutable snapshots make reads wait-free without unacceptable birth cost?
- Can Swiss-style control bytes be combined safely with sharded writers?
- Do epochs or hazards reduce remove cost enough to repay pinning overhead?
- Is resizing avoidable through capacity planning or segmented growth?
- Which design minimizes cache misses and false sharing at 1M registrations?

The winning design must be explainable, model-checked, Miri-clean, allocation
measured, and faster on representative workloads—not merely novel.

