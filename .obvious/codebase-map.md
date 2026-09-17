# Codebase Map — bombay-address

Folder-level overview, depth-capped at 2. Verified 2026-09-17.

| Path | Purpose |
|---|---|
| `crates/address/` | The product: published library `bombay-address` v0.2.0 (lib name `address`), + CHANGELOG. |
| `crates/address/src/` | `lib.rs` — public API (`AddressSpace`, `Lease`, `Resolved`, error types) with the documented key/ownership contracts; `table.rs` — open-addressing table with generation-gated removal. |
| `crates/address/tests/` | Integration tests: `semantics.rs`, `reclamation.rs`, `reclamation_alloc.rs` (counting allocator), `loom.rs` + `loom_model.rs` (cfg(loom)-gated interleaving models). |
| `crates/address/benches/` | `address_space.rs` — criterion `resolve_hit` benchmark at 1,024 / 65,536 entries. |
| `crates/address-perf/` | `address-perf` executable: claims 65,536 addresses, runs 5,000,000 resolves, prints `SCORE` / `THROUGHPUT_OPS` / `RESOLVE_NS`. |
| `research/address-tests/` | Detached verification-campaign crate (NOT a workspace member). `src/lib.rs` reference model + fuzz entries; `tests/` proptest/loom/fuzz-replay/stress/panic-safety/lifecycle/linearizability; `fuzz/` cargo-fuzz targets. `VERIFICATION.md` = full campaign report incl. FINDINGS 001–005. Run via `--manifest-path`. |
| `docs/` | Published docs content (`index.mdx`, `correctness.mdx`, `leases.mdx`) + `research-log.md`; `docs.json` at repo root is the docs.page config. |
| `.github/workflows/` | `checks.yml` (CI = `nix flake check` on PRs to main), `release-plz.yml` (release automation on main), `prune-release-plz-branches.yml`. |
| root | `Cargo.toml` (workspace; lints deny `clippy::all`, warn `pedantic`), `rust-toolchain.toml` (Rust 1.96.0), `flake.nix` (crane gate: build/fmt/clippy/nextest/doctest/doc/audit/deny + coverage package), `deny.toml`, `audit.toml`, `release-plz.toml`, `AGENTS.md`, `README.md`. |
