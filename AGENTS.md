# Repository Guidelines

## Project Structure & Module Organization

The workspace contains the published library in `crates/address/` (`src/`, `tests/`, and `benches/`) and a performance executable in `crates/address-perf/`. Documentation lives in `docs/`.

`research/address-tests/` is a detached crate for property, Loom, Miri, stress, and fuzz testing. Root workspace commands exclude it; see its `VERIFICATION.md`.

## Build, Test, and Development Commands

Use Nix for every task. Enter with `nix develop`; run all Cargo commands there. `nix flake check -L` is the required build, format, Clippy, nextest, doctest, docs, audit, and license gate. `nix build .#coverage -L` produces HTML coverage. Targeted commands include `cargo test --workspace`, `cargo bench -p bombay-address`, and `cargo test --manifest-path research/address-tests/Cargo.toml`.

## Engineering Principles

High performance is mandatory. Before choosing an algorithm or data structure, review current primary literature, including relevant arXiv papers; define the workload and invariants; and compare the best applicable approaches. Benchmark representative workloads and retain reproducible evidence.

Never assume correctness. Validate every behavior change with suitable unit, property, stress, Loom, Miri, fuzz, and benchmark coverage. Check current stable Rust patterns and standard-library capabilities before adding custom machinery; record non-obvious rationale.

After every pass, review the entire crate for unnecessary abstractions, duplication, allocations, dependencies, and code paths. Distill it to the smallest design preserving every feature, invariant, safety property, and measured performance characteristic.

## Coding Style & Naming Conventions

Use rustfmt defaults (four-space indentation) and idiomatic Rust naming: `snake_case` for modules, functions, and tests; `CamelCase` for types and traits; `SCREAMING_SNAKE_CASE` for constants. The workspace denies Clippy's `all` group and warns on `pedantic`; keep all targets warning-free. Document public APIs and preserve the concurrency and key-contract guarantees described in `crates/address/src/lib.rs`.

## Testing Guidelines

Place focused integration tests in `crates/address/tests/`; name tests after observable behavior, such as `duplicate_claim_preserves_the_live_endpoint`. Put adversarial or long-running verification in `research/address-tests/`. Concurrency changes require relevant ownership, reclamation, reentrancy, or interleaving regression coverage.

## Commit & Pull Request Guidelines

Use concise Conventional Commit subjects such as `feat(reclamation): ...` or `test(reclamation): ...`. Keep commits scoped and green. Pull requests must explain behavior, affected invariants, verification commands, and linked issues. Include benchmark or coverage results when relevant.
