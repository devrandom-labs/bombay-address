# Addresspass research rules

Addresspass is a generic concurrent address space. It owns address claim,
resolution, generation-safe release, and replacement mechanics. It must not
know about actors, behaviorpass, actorpass, messages, mailboxes, Tokio,
supervision, watching, timers, or service discovery.

The public semantics and frozen tests are the contract. Optimize only after a
correctness test or measurable hypothesis exists. Never weaken a correctness
gate or benchmark workload to improve a score. Prefer safe Rust. Any `unsafe`
must include a documented invariant, a focused Miri test, a Loom model where
concurrency is involved, and evidence that safe candidates cannot meet the
measured target.

Use primary sources when researching algorithms: original papers, official
implementation notes, language/library documentation, and source code. Record
citations, applicability, rejected assumptions, and failed experiments in
`docs/research-log.md`. A result that does not improve the representative
workload is still a useful result and must be recorded.

Run `cargo fmt --all -- --check`, `cargo test --workspace`, and
`cargo clippy --workspace --all-targets -- -D warnings` before every coherent
commit. Run `.auto/checks.sh` before accepting an autoresearch experiment.

