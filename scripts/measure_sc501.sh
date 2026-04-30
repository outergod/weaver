#!/usr/bin/env bash
# scripts/measure_sc501.sh — slice 005 T033 driver
#
# Measures `weaver save → buffer/dirty=false` wall-clock latency over
# N iterations and reports min / median / p95 / max in milliseconds.
# The per-iteration window is captured by the existing T022 e2e test
# (`tests/e2e/buffer_save_dirty.rs::dirty_save_flips_dirty_to_false_and_persists_disk`),
# which prints `[sc-501] weaver save → buffer/dirty=false in <Duration>`
# under `--nocapture`. This script invokes that test N times and
# aggregates the printed durations.
#
# The per-iteration window is save-only (timer starts immediately
# before `weaver save` dispatch; ends when the AllFacts observer
# sees `buffer/dirty=false`). Process-spawn / build overhead lives
# OUTSIDE the window and does not pollute the measurement.
#
# Stats go to stdout; per-iteration progress + errors go to stderr.
# Operator judges median against the SC-501 ≤500 ms budget; this
# script reports stats but does NOT make the pass/fail call (per
# feedback_operator_involvement_interactive_checks.md).
#
# Usage:
#   scripts/measure_sc501.sh                 # N=20 (default)
#   N=50 scripts/measure_sc501.sh            # custom sample count
#   scripts/measure_sc501.sh > stats.txt     # redirect stats; progress on stderr

set -eu

N="${N:-20}"
TEST_NAME="dirty_save_flips_dirty_to_false_and_persists_disk"
SAMPLES_FILE=$(mktemp)
trap 'rm -f "$SAMPLES_FILE"' EXIT

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

echo "[sc-501] building binaries + test runner..." >&2
cargo build --quiet -p weaver_core --bin weaver
cargo build --quiet -p weaver-git-watcher --bin weaver-git-watcher
cargo build --quiet -p weaver-buffers --bin weaver-buffers
cargo test --test buffer_save_dirty --quiet --no-run

echo "[sc-501] running $N iterations of ${TEST_NAME}..." >&2
for i in $(seq 1 "$N"); do
    if ! output=$(cargo test --test buffer_save_dirty --quiet -- \
            --exact "$TEST_NAME" --nocapture 2>&1); then
        echo "[sc-501] ERROR iteration $i: cargo test failed" >&2
        echo "$output" >&2
        exit 1
    fi
    line=$(echo "$output" | grep -m1 '\[sc-501\]' || true)
    if [ -z "$line" ]; then
        echo "[sc-501] ERROR iteration $i: no [sc-501] line in test output" >&2
        echo "$output" >&2
        exit 1
    fi
    echo "$line" >> "$SAMPLES_FILE"
    printf '[sc-501]   iter %2d/%d: %s\n' "$i" "$N" "$line" >&2
done

python3 - "$SAMPLES_FILE" <<'PY'
import re
import sys

path = sys.argv[1]
samples = []
with open(path) as f:
    for line in f:
        m = re.search(r"in\s+([0-9.]+)(ms|s|µs|us|ns)\b", line)
        if not m:
            continue
        n, u = float(m.group(1)), m.group(2)
        factor = {"s": 1000.0, "ms": 1.0, "µs": 0.001, "us": 0.001, "ns": 1e-6}[u]
        samples.append(n * factor)

if not samples:
    print("ERROR: no samples parsed", file=sys.stderr)
    sys.exit(1)

samples.sort()
n = len(samples)
median = (samples[n // 2] + samples[(n - 1) // 2]) / 2.0
p95 = samples[max(0, int(round(0.95 * (n - 1))))]
print()
print(f"=== SC-501 timing (N={n}) ===")
print(f"  min:    {min(samples):8.3f} ms")
print(f"  median: {median:8.3f} ms   (SC-501 budget: <= 500 ms median)")
print(f"  p95:    {p95:8.3f} ms")
print(f"  max:    {max(samples):8.3f} ms")
print()
print(f"  median vs budget: {'within budget' if median <= 500.0 else 'OVER BUDGET'}")
print()
print("Operator judges; this script reports stats. Per")
print("feedback_operator_involvement_interactive_checks.md, the")
print("pass/fail call rests with the operator, not this script.")
PY
