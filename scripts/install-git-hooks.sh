#!/usr/bin/env sh
# scripts/install-git-hooks.sh — install repo-provided git hooks.
#
# Per L2 constitution Amendment 6 (code quality gates). Idempotent;
# refuses to overwrite an existing hook unless --force.
#
# Usage:
#   scripts/install-git-hooks.sh            # install (error if a hook exists)
#   scripts/install-git-hooks.sh --force    # install, overwriting any existing hook
#   scripts/install-git-hooks.sh --uninstall # remove repo-installed hooks

set -eu

FORCE=0
UNINSTALL=0
for arg in "$@"; do
    case "$arg" in
        --force) FORCE=1 ;;
        --uninstall) UNINSTALL=1 ;;
        -h|--help)
            cat <<'USAGE'
scripts/install-git-hooks.sh — install pre-commit hook.

Options:
  --force      Overwrite an existing hook.
  --uninstall  Remove hooks that match the repo-provided versions.
  -h, --help   Show this message.

Bypass temporarily: git commit --no-verify
USAGE
            exit 0
            ;;
        *)
            echo "scripts/install-git-hooks.sh: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

# Resolve the repo root from the script's location.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GIT_DIR="$REPO_ROOT/.git"

if [ ! -d "$GIT_DIR" ]; then
    # Support worktrees (.git is a file pointing at the real gitdir).
    if [ -f "$GIT_DIR" ]; then
        GIT_DIR="$(cd "$REPO_ROOT" && git rev-parse --git-dir)"
    else
        echo "install-git-hooks: not a git repository: $REPO_ROOT" >&2
        exit 1
    fi
fi

HOOKS_SRC="$SCRIPT_DIR/hooks"
HOOKS_DST="$GIT_DIR/hooks"
mkdir -p "$HOOKS_DST"

install_hook() {
    name="$1"
    src="$HOOKS_SRC/$name"
    dst="$HOOKS_DST/$name"
    if [ ! -f "$src" ]; then
        echo "install-git-hooks: missing source hook: $src" >&2
        exit 1
    fi
    if [ -e "$dst" ] && [ "$FORCE" -eq 0 ]; then
        echo "install-git-hooks: $dst already exists; pass --force to overwrite" >&2
        exit 1
    fi
    cp "$src" "$dst"
    chmod +x "$dst"
    echo "  installed: $dst"
}

uninstall_hook() {
    name="$1"
    src="$HOOKS_SRC/$name"
    dst="$HOOKS_DST/$name"
    if [ -f "$dst" ] && [ -f "$src" ] && cmp -s "$src" "$dst"; then
        rm "$dst"
        echo "  removed: $dst"
    elif [ -e "$dst" ]; then
        echo "  skipped (diverged from repo version): $dst"
    fi
}

if [ "$UNINSTALL" -eq 1 ]; then
    echo "scripts/install-git-hooks.sh: uninstalling"
    uninstall_hook pre-commit
    echo "Done."
else
    echo "scripts/install-git-hooks.sh: installing"
    install_hook pre-commit
    cat <<'NEXT'
Done. The pre-commit hook will run cargo lint + cargo fmt-check on
every commit. Bypass temporarily with `git commit --no-verify`.
NEXT
fi
