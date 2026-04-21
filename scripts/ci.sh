#!/usr/bin/env sh
# scripts/ci.sh — local CI chain (per L2 constitution Amendment 6).
#
# Runs the same gates CI enforces: clippy, rustfmt, build, test.
# Exits non-zero on first failure.
#
# Usage: scripts/ci.sh [--no-test]

set -eu

NO_TEST=0
for arg in "$@"; do
    case "$arg" in
        --no-test) NO_TEST=1 ;;
        -h|--help)
            cat <<'USAGE'
scripts/ci.sh — run local quality gates in sequence.

Steps:
  1. cargo lint        (clippy --all-targets --workspace -- -D warnings)
  2. cargo fmt-check   (fmt --all -- --check)
  3. cargo build --workspace
  4. cargo test --workspace   (skip with --no-test)

Options:
  --no-test   Skip the cargo test step (quicker pre-commit check).
  -h, --help  Show this message.
USAGE
            exit 0
            ;;
        *)
            echo "scripts/ci.sh: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

echo "[ci] cargo lint"
cargo lint

echo "[ci] cargo fmt-check"
cargo fmt-check

echo "[ci] cargo build --workspace"
cargo build --workspace

if [ "$NO_TEST" -eq 0 ]; then
    echo "[ci] cargo test --workspace"
    cargo test --workspace
fi

echo "[ci] OK"
