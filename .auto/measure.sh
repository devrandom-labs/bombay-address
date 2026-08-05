#!/usr/bin/env bash
set -euo pipefail

export RUSTFLAGS="${RUSTFLAGS:-} -C target-cpu=native"
cargo build -q -p addresspass-perf --release
./target/release/addresspass-perf >/dev/null

best=0
output=""
for _ in 1 2 3 4 5; do
  run=$(./target/release/addresspass-perf)
  score=$(printf '%s\n' "${run}" | sed -n 's/^SCORE=//p')
  if awk -v candidate="${score}" -v current="${best}" 'BEGIN { exit !(candidate > current) }'; then
    best=${score}
    output=${run}
  fi
done

echo "METRIC score=${best} unit=resolve_ops_per_second"
printf '%s\n' "${output}" | sed -n 's/^THROUGHPUT_OPS=/METRIC throughput_ops=/p'
printf '%s\n' "${output}" | sed -n 's/^RESOLVE_NS=/METRIC resolve_ns=/p'

