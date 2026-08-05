#!/usr/bin/env bash
set -euo pipefail

test -f .auto/prompt.md
test -x .auto/checks.sh
test -x .auto/measure.sh

