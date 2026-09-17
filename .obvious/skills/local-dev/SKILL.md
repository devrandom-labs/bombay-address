---
name: local-dev
description: Stand up and verify a working local dev environment for bombay-address (pure Rust workspace — no services, no ports, no env vars)
---

# Local dev — bombay-address

Durable record of the 2026-09-17 onboarding run (sandbox snapshot
`i2dreh1oj0k139vseaq7y`, captured 2026-09-17T15:29:11.468Z).

## What this repo needs

Rust 1.96.0 only. No services, no ports, no required env vars, no Docker. The Nix flake
is the canonical full gate, but plain cargo via rustup covers every lane equivalently.

## Setup from a clean sandbox

1. `curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.96.0 -c rustfmt,clippy`
   (inside the repo, rustup then auto-honors `rust-toolchain.toml`).
2. `. "$HOME/.cargo/env"`
3. `cargo install cargo-audit cargo-deny --locked` (~2 min) — only needed for the
   audit/deny lanes.

## Verify (all validated 2026-09-17; results table in obvious.md)

```bash
cargo build --workspace --all-targets
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                       # 32 passed incl. 6 doctests
LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS='--cfg loom' cargo test --workspace --test loom --test loom_model --release   # 4 passed
cargo bench -p bombay-address --bench address_space   # ~42-47 ns
cargo run -p address-perf --release           # SCORE ~22M ops/s
cargo doc --workspace --no-deps
cargo audit --file Cargo.lock && cargo deny check
cargo test --manifest-path research/address-tests/Cargo.toml --release   # 85 passed, 8 ignored
```

The 8 ignored research tests are the documented FINDING-001/003/004/005 reproducers —
expected, do not "fix" them.

## Known gotchas (do not re-debug)

- `tests/loom*.rs` are `cfg(loom)`-gated: they compile but run 0 tests without
  `RUSTFLAGS='--cfg loom'`.
- `cargo bench -p bombay-address` without `--bench address_space` breaks: criterion
  args reach the lib unittest binary ("Unrecognized option: 'warm-up-time'").
- The research loom lane (`research/address-tests --test loom_model` under
  `--cfg loom`) does NOT compile — stale against the v0.2.0 `Resolved` API
  (E0308/E0277, pre-existing upstream). The workspace loom lane is the green one.
- Any research-crate build rewrites `research/address-tests/Cargo.lock` (stale 0.1.0
  path-dep pin) — restore with `git checkout -- research/address-tests/Cargo.lock`.
- Miri needs a nightly toolchain (`nix develop .#miri`); not validated in this sandbox.
  The flake/CI gate does not include Miri.
