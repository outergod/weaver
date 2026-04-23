# weaver-git-watcher

The first non-editor service actor on the Weaver bus. Observes one
local git repository and publishes authoritative `repo/*` facts
(dirty / head-commit / working-copy state) under a structured
`ActorIdentity::Service`.

## Role

`weaver-git-watcher` realises the coordination-substrate pivot
(constitution v0.2 §17): an external actor participates on the bus
as a first-class peer alongside the core and its in-process
behaviors. See `specs/002-git-watcher-actor/` for the full
specification, `quickstart.md` for a three-terminal walkthrough, and
`data-model.md` for the fact families published.

## Usage

```bash
weaver-git-watcher <REPOSITORY-PATH> [OPTIONS]
```

Core must be running first (via `weaver run`) so the watcher has a
bus socket to connect to. Order of startup (watcher vs. TUI) is
otherwise free.

### Common flags

- `--socket <PATH>` — bus socket path. Reads `WEAVER_SOCKET` env var
  if unset; falls back to `$XDG_RUNTIME_DIR/weaver.sock` then
  `/tmp/weaver.sock`. Parity with `weaver` / `weaver-tui`.
- `--poll-interval <DURATION>` — observation cadence. Humantime-
  parsed (`250ms`, `1s`, …). Default `250ms`. Must be non-zero.
- `--output json|human` — controls `--version` rendering AND runtime
  tracing format. Default `human`.
- `-v` / `-vv` / `-vvv` — `info` / `debug` / `trace` log level.
  Overridden by `RUST_LOG` when set.
- `--version` — print build provenance (commit, dirty flag, profile,
  rustc, bus protocol, service-id) and exit.

### Quick example

```bash
# Terminal 1: core
weaver run

# Terminal 2: create a throwaway repo and watch it
REPO=$(mktemp -d /tmp/weaver-watch.XXXXXX)
(cd "$REPO" && git init -b main -q && \
  git config user.email demo@example.invalid && \
  git config user.name "Demo" && \
  echo hello > a.txt && git add . && git commit -q -m initial)
weaver-git-watcher "$REPO"

# Terminal 3: see the repo render in the TUI
weaver-tui
```

The watcher's startup log names its per-invocation instance UUID and
the repository entity reference; both appear in the TUI's
Repositories section:

```
/tmp/weaver-watch.XXXXXX  [on main] clean
  head: 4f7a8e3b...
  by service git-watcher (inst 2e1a4f8b), event 3, 0.31s ago
```

Touch a tracked file → the TUI flips to `dirty` within one poll
cadence. Commit → `clean` + updated `head:` SHA. Check out a raw
SHA → `[detached <short-sha>]`. All within 500 ms by spec
(SC-002).

## Published facts

Under the repository entity (keyed by the canonical working-tree
root):

| Attribute | Type | Semantics |
|---|---|---|
| `repo/path` | String | Canonical working-tree root. |
| `repo/dirty` | Bool | Index-or-working-tree differs from HEAD. Untracked-only is clean (Clarification Q5). |
| `repo/head-commit` | String | Lowercase hex-encoded object id as produced by `gix::rev_parse_single("HEAD").to_hex()` — 40 chars for SHA-1 repositories, 64 for SHA-256 repositories. Absent when HEAD is unborn. |
| `repo/state/on-branch` | String | Branch name when HEAD points at `refs/heads/<name>`. |
| `repo/state/detached` | String | Commit SHA when HEAD is detached. |
| `repo/state/unborn` | String | Intended branch name for an empty repo. |
| `repo/observable` | Bool | `false` while the watcher is degraded. |

At most one `repo/state/*` attribute is asserted per repository at
any time — the mutex invariant (see
`docs/07-open-questions.md §26`) is property-tested in
`git-watcher/tests/mutex_invariant.rs`.

Under a separate entity keyed by the watcher's instance UUID:

| Attribute | Type | Semantics |
|---|---|---|
| `watcher/status` | String | Mirrors `LifecycleSignal` (`started` / `ready` / `degraded` / …). |

## Exit codes

Per `specs/002-git-watcher-actor/contracts/cli-surfaces.md`:

| Code | Meaning |
|---|---|
| 0 | Clean shutdown (SIGTERM / SIGINT; facts retracted). |
| 1 | Startup failure (path not a repo, bare repo, transient op in progress, bootstrap observation failed, malformed `--poll-interval`). |
| 2 | Bus unavailable. |
| 3 | `authority-conflict` or `not-owner` rejection from the core. Another watcher already owns `repo/*` for the target entity, or the watcher tried to retract a key owned by a different connection. |
| 10 | Unclassified internal error, including protocol-violation rejections other than the two listed above — specifically `identity-drift` and `invalid-identity`. A correctly-behaving watcher never produces these; receiving one indicates a bug in the watcher itself. Follow-up: reclassify these as fatal and map to a dedicated exit code so operator automation can distinguish protocol misuse from generic internal faults. |

## Behavior under failure

- **Watcher crash (SIGKILL)**: the core detects the connection
  drop and retracts every `repo/*` fact the watcher published — no
  stale state in the fact store.
- **Repo gone (permissions, corruption)**: observation fails →
  `Lifecycle(Degraded)` + `repo/observable=false` emitted once on
  the transition edge. Subsequent failed polls stay at debug log
  level. First successful observation re-publishes `Ready` +
  `repo/observable=true`.
- **Second watcher on the same repo**: the second instance receives
  `Error { category: "authority-conflict", ... }` during its
  bootstrap publish and exits with code 3.

## License

AGPL-3.0-or-later. See the top-level `LICENSE`.
