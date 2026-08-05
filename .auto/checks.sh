#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
LOOM_MAX_PREEMPTIONS=3 RUSTFLAGS="--cfg loom" \
  cargo test -p addresspass --test loom --release

base=$(cat .auto/BASELINE 2>/dev/null || true)
if [ -n "${base}" ]; then
  frozen=(
    .auto/checks.sh
    .auto/measure.sh
    crates/addresspass/tests/semantics.rs
    crates/addresspass/benches/address_space.rs
    crates/addresspass-perf
  )
  git diff --quiet "${base}" -- "${frozen[@]}" || {
    echo "CHECK FAIL: frozen oracle or measurement surface changed"
    exit 1
  }
fi

echo "CHECK OK"

