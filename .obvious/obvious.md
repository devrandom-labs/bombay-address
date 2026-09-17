# bombay-address — Agent Guide

**Repo:** devrandom-labs/bombay-address
**What:** A typed concurrent address space Rust library with exclusive, generation-safe
ownership. The published crate is `bombay-address` (library name `address`): actor-style
claim → resolve → release over an `AddressSpace`, with `Lease` as the release authority
and `Resolved<E>` as the opaque endpoint snapshot. Licensed MIT OR Apache-2.0.

## Stack

- **Runtime:** Rust 1.96.0 stable, pinned in `rust-toolchain.toml` (components rustfmt +
  clippy; edition 2024). No async runtime; only safe-Rust caller-facing API.
- **Package manager:** Cargo workspace with 2 members; `rustup` provides the toolchain.
  A Nix flake (nixos-unstable + fenix + crane) wraps the same commands as the canonical
  gate (`nix flake check -L`) — that gate is what CI runs, and it needs Nix installed.
- **Crates:**
  - `crates/address` — `bombay-address` v0.2.0, the published library (deps: `parking_lot`
    0.12; `loom` 0.7 under `cfg(loom)`; criterion 0.8 benches).
  - `crates/address-perf` — `address-perf`, a release-mode perf executable (publish = false).
- **Detached crate:** `research/address-tests` — adversarial verification campaign
  (reference model, proptest, loom, fuzz replay, stress, Miri). NOT a workspace member;
  always run it via `--manifest-path research/address-tests/Cargo.toml`.
- **Services / ports / env vars:** none. No Docker, no database, no required env vars.
  Optional: `LOOM_MAX_PREEMPTIONS`, `RUSTFLAGS='--cfg loom'` (loom lane).

## Commands (all validated in this sandbox on 2026-09-17)

| Task | Command |
|---|---|
| Build (all targets) | `cargo build --workspace --all-targets` |
| Format check | `cargo fmt --check` |
| Lint / typecheck | `cargo clippy --workspace --all-targets -- --deny warnings` |
| Tests + doctests | `cargo test --workspace` |
| Loom concurrency lane | `LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS='--cfg loom' cargo test --workspace --test loom --test loom_model --release` |
| Benchmarks | `cargo bench -p bombay-address --bench address_space` |
| Perf executable | `cargo run -p address-perf --release` |
| Docs | `cargo doc --workspace --no-deps` |
| Security audit | `cargo audit --file Cargo.lock` |
| Licenses / bans | `cargo deny check` |
| Research campaign suite | `cargo test --manifest-path research/address-tests/Cargo.toml --release` |
| Full gate (needs Nix; runs in CI) | `nix flake check -L` |
| HTML coverage (needs Nix) | `nix build .#coverage -L` |
| Miri (needs nightly via `nix develop .#miri`) | `cargo miri test --manifest-path research/address-tests/Cargo.toml` |

## Codebase map

See `codebase-map.md` (single table, depth 2).

## Local Verification Summary (2026-09-17, sandbox i2dreh1oj0k139vseaq7y)

| Lane | Result |
|---|---|
| `cargo build --workspace --all-targets` | PASS — finished in 22.7s |
| `cargo fmt --check` | PASS — clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS — clean |
| `cargo test --workspace` | PASS — 32 passed, 0 failed (14 unit, 8 reclamation, 1 reclamation_alloc, 3 semantics, 6 doctests) |
| Loom lane (workspace, depth 3) | PASS — 4 passed |
| `cargo bench` resolve_hit | PASS — ~42 ns @1,024 entries, ~47 ns @65,536 |
| `address-perf --release` | PASS — SCORE=22274042, 22.27M resolves/s, 44.9 ns/resolve |
| `cargo doc --workspace --no-deps` | PASS |
| `cargo audit` | PASS — 0 vulnerabilities |
| `cargo deny check` | PASS — advisories, bans, licenses, sources ok |
| Research suite (release) | PASS — 85 passed, 8 ignored (documented FINDING reproducers) |

**Primary user flow verified:** the README example — `AddressSpace::new → claim →
resolve → drop(lease) → resolve == None` — exercised end-to-end by the 6 doctests, and
at scale by `address-perf` (65,536 claims + 5,000,000 resolves, exit 0).

## Snapshot

- **Snapshot ID:** `i2dreh1oj0k139vseaq7y` — captured 2026-09-17T15:29:11.468Z.
- Contains: rustup with Rust 1.96.0 (rustfmt, clippy), cargo-audit + cargo-deny in
  `~/.cargo/bin`, and warm `target/` build caches for both the workspace and the
  research crate.

## Gotchas

1. **Loom tests are `cfg(loom)`-gated.** Plain `cargo test --workspace` compiles
   `tests/loom.rs` / `tests/loom_model.rs` but runs 0 tests. Set
   `RUSTFLAGS='--cfg loom'` for that lane.
2. **`cargo bench` needs `--bench address_space`.** Bare `cargo bench -p bombay-address`
   also runs the lib unittest binary and criterion args leak into it
   ("Unrecognized option: 'warm-up-time'").
3. **Research crate is stale against v0.2.0 (pre-existing, upstream — do not re-debug):**
   - `research/address-tests/tests/loom_model.rs` still expects `resolve → Option<E>`;
     since #8 `resolve` returns `Option<Resolved<E>>`, so the research loom lane
     FAILS TO COMPILE (E0308/E0277). The workspace loom lane is green. Fix is upstream.
   - `research/address-tests/Cargo.lock` pins the path dep as `bombay-address 0.1.0`;
     cargo rewrites it (dirty tree) on any research build — restore it with
     `git checkout -- research/address-tests/Cargo.lock`.
4. **Nix is the canonical gate** (`nix flake check -L`) but is not installed in the
   sandbox; the cargo commands above are the verified equivalents, and CI runs the Nix
   gate on every PR to main.
5. Workspace lints deny `clippy::all` and warn on `clippy::pedantic` — keep every
   target warning-free (AGENTS.md).

## Conventions (from AGENTS.md)

- Conventional Commit subjects (`feat(reclamation): ...`, `test(loom): ...`).
- Integration tests in `crates/address/tests/`, named after observable behavior;
  adversarial or long-running verification goes to `research/address-tests/`.
- Concurrency changes need ownership/reclamation/reentrancy/interleaving coverage.
- rustfmt defaults; document public APIs; see `AGENTS.md` for full guidelines.
