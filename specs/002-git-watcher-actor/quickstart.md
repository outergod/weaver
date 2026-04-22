# Quickstart: Git-Watcher Actor

Build, run, and verify the git-watcher slice end-to-end. Validates spec Success Criteria SC-001 through SC-006 and establishes non-regression of slice 001 acceptance (part of SC-006).

## Prerequisites

- Rust 1.94.0 toolchain (pinned via `rust-toolchain.toml` + `fenix` in `flake.nix`).
- A POSIX-compatible terminal (Linux or macOS).
- `git` installed and on `$PATH` (for repository setup in the quickstart; the watcher itself uses `gix`, not `git`).
- A throwaway git repository to watch — the quickstart creates a temporary one under `/tmp/`.

## Build

From the repository root:

```bash
cargo build --workspace
```

First build fetches the new `gix` and `uuid` dependencies and may take a few minutes (pure Rust; no C compile step). Subsequent builds are incremental.

Optimized build:

```bash
cargo build --workspace --release
```

## Run

The slice requires three terminal windows — one for the core, one for the watcher, one for the TUI.

### Terminal 1 — start the core

```bash
cargo run --bin weaver -- run
```

Structured logs end at `Lifecycle::Ready`. Core listens on `$XDG_RUNTIME_DIR/weaver.sock` by default.

### Terminal 2 — prepare a throwaway repository and start the watcher

```bash
# Create a repository to watch.
REPO=$(mktemp -d /tmp/weaver-quickstart.XXXXXX)
cd "$REPO"
git init -b main -q
git config user.email weaver-quickstart@example.invalid
git config user.name  "Weaver Quickstart"
echo 'hello' > hello.txt
git add hello.txt
git commit -q -m "initial commit"

# Start the watcher against it.
cargo run --bin weaver-git-watcher -- "$REPO"
```

Watcher startup logs include the instance UUID, the resolved socket, and the poll interval (default 250 ms). Look for:

```
[INFO] connected to core (bus protocol v0.2.0)
[INFO] published initial state:
         repo/path            = /tmp/weaver-quickstart.XXXXXX
         repo/head-commit     = <sha>
         repo/state/on-branch = main
         repo/dirty           = false
         repo/observable      = true
         watcher/status       = ready
```

### Terminal 3 — start the TUI

```bash
cargo run --bin weaver-tui
```

TUI should display a **Repositories** section populated with the watched repo. Buffers section is empty (no `simulate-edit` yet).

```
┌─ Weaver TUI ────────────────────────────────────────────────────────┐
│ Connection: ready (bus v0.2.0, core 0.2.0+...)                      │
│                                                                     │
│ Buffers: (none)                                                     │
│                                                                     │
│ Repositories:                                                       │
│   /tmp/weaver-quickstart.XXXXXX  [on main] clean                    │
│     head: <sha>                                                     │
│     by service git-watcher (inst 2e1a4f8b...), event 3, 0.31s ago   │
│                                                                     │
│ Commands: [e]dit  [c]lean  [i]nspect  [r]econnect  [q]uit           │
└─────────────────────────────────────────────────────────────────────┘
```

## Verify the slice

### SC-001 — cold start observation

Shut down the watcher (Ctrl-C in Terminal 2), then restart it:

```bash
cargo run --bin weaver-git-watcher -- "$REPO"
```

**Pass criterion**: within **1 second** of the watcher's startup line appearing in Terminal 2, the TUI's Repositories section reflects the repository's current state. Measure with the timestamps in Terminal 2's startup logs vs. your TUI observation.

### SC-002 — external mutation → TUI propagation

With all three processes running and the watcher reporting clean state:

```bash
# In any terminal:
cd "$REPO"
echo 'modified' > hello.txt
```

**Pass criterion**: within **500 ms** of the `echo` completing, the TUI's Repositories section shows the repo as `dirty`. Repeat with:

- `git add hello.txt` (stage the change) — should remain `dirty` (per Q5 semantics: index-vs-HEAD counts).
- `git commit -q -m "update"` — should become `clean`; `repo/head-commit` updates; `repo/state/on-branch` unchanged.
- `git checkout -q <prior-sha>` — should show `[detached <short-sha>]`. The transition retracts `repo/state/on-branch` and asserts `repo/state/detached` atomically.
- `git checkout -q main` — should return to `[on main]`.

Each transition is visible in the TUI within the 500 ms budget.

### SC-003 — structured actor identity on inspection

From a fourth terminal (or pause the TUI):

```bash
# Inspect the repo/dirty fact.
cargo run --bin weaver -- inspect <entity-id>:repo/dirty --output=json | jq .
```

The entity ID appears in the TUI's rendering and in the watcher's startup log. Output should be:

```json
{
  "fact": { "entity": <N>, "attribute": "repo/dirty" },
  "source_event": <id>,
  "asserting_service": "git-watcher",
  "asserting_instance": "2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90",
  "asserted_at_ns": <timestamp>,
  "trace_sequence": <seq>
}
```

**Pass criterion**: `asserting_service` and `asserting_instance` are present and match the watcher's startup output; the result does NOT contain an opaque `External(...)` string anywhere.

For comparison, a slice-001 behavior-authored fact still renders with `asserting_behavior`:

```bash
cargo run --bin weaver -- simulate-edit 1
cargo run --bin weaver -- inspect 1:buffer/dirty --output=json | jq .
```

```json
{
  "fact": { "entity": 1, "attribute": "buffer/dirty" },
  "source_event": <id>,
  "asserting_behavior": "core/dirty-tracking",
  "asserted_at_ns": <timestamp>,
  "trace_sequence": <seq>
}
```

**Pass criterion**: the two JSON shapes coexist cleanly; consumers see `asserting_behavior` for behaviors and `asserting_service` + `asserting_instance` for services.

### SC-004 — three-process scenario runs from a documented procedure

The above terminal-by-terminal procedure **is** the documented procedure. The slice's automated counterpart is `tests/e2e/git_watcher_attach.rs`.

**Pass criterion**: a fresh reader follows the steps, gets a working three-process system, and observes the expected fact propagation without needing to modify code or configuration beyond the `REPO=$(mktemp -d ...)` line.

### SC-005 — watcher disconnect retracts its facts

With the watcher running and the TUI showing repository state, stop the watcher (Ctrl-C in Terminal 2).

**Pass criterion**:

- Within interactive latency, the TUI's Repositories section either (a) empties out (facts retracted), or (b) shows `[stale]` markers with the last-known state plus `observability lost`.
- Running `weaver status --output=json` in a fourth terminal shows no asserted `repo/*` facts (option a) OR shows `repo/observable = false` alongside stale state facts (option b) — whichever convention the watcher uses for its final retract-sequence.
- No fact authored by the now-dead watcher instance is displayed as current without an observable indication of staleness.

Implementation is free to pick (a) or (b) at the watcher's shutdown handler; whichever shape ships must be consistent with FR-014.

### SC-006 — non-regression of slice 001

Run the full test suite:

```bash
cargo test --workspace
```

**Pass criterion**: every slice-001 acceptance scenario continues to pass — buffer-edit → dirty-state propagation, inspection, status, version output, disconnect handling. Slice 001's behavior-authored facts render correctly under the new structured-actor wire shape. The e2e test suite includes the three new files (`git_watcher_attach.rs`, `git_watcher_transitions.rs`, `structured_actor_inspection.rs`).

If any slice-001 test regresses, the wire-change migration is incomplete; the regression must be fixed before merge.

## Manual exploration

```bash
# Observe the watcher's state transitions in the trace (via inspection loop).
for attr in repo/dirty repo/head-commit repo/state/on-branch; do
  cargo run --bin weaver -- inspect <entity>:$attr --output=json | jq -r '
    "\(.fact.attribute): svc=\(.asserting_service) inst=\(.asserting_instance[:8])"'
done

# Tune the watcher's polling interval for interactive debugging.
cargo run --bin weaver-git-watcher -- "$REPO" --poll-interval=50ms

# Watch the watcher's tracing output at debug level.
RUST_LOG=weaver_git_watcher=debug cargo run --bin weaver-git-watcher -- "$REPO"

# Verify the bus protocol version.
cargo run --bin weaver -- --version | grep 'bus protocol'
# → bus protocol: v0.2.0
```

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Watcher exits with `WEAVER-GW-001 repo not observable` | Path is not a git repository | `cd $REPO && git init -b main -q` |
| Watcher exits with `WEAVER-GW-002 bus unavailable` | Core not running or wrong socket path | Start core; check `$WEAVER_SOCKET` / `--socket` flag |
| Watcher exits with `WEAVER-GW-003 authority conflict` | Another watcher already observing this repo | Kill the first instance, or pick a different repo for this one |
| TUI shows `UNAVAILABLE` immediately | Core not running | Start core first |
| TUI Repositories section is empty but watcher is running | Watcher connected to a different core / socket | Ensure all three processes agree on `$WEAVER_SOCKET` |
| `repo/dirty` stays `true` after `git commit` | Polling interval + timing; wait one poll cycle (≤ 250 ms by default) | If persistently stuck, re-check with `weaver inspect` |
| `weaver status` shows slice-001 facts but no `repo/*` facts | Watcher never handshake-completed, or version mismatch | Check watcher log for `version-mismatch` error; rebuild both binaries from this slice's source |
| `cargo test` hangs on a git-watcher e2e test | Temporary socket collision | Stop other weaver processes; `lsof $XDG_RUNTIME_DIR/weaver.sock` |

## References

- `specs/002-git-watcher-actor/spec.md` — full specification.
- `specs/002-git-watcher-actor/plan.md` — implementation plan and Constitution Check.
- `specs/002-git-watcher-actor/research.md` — library and cadence decisions.
- `specs/002-git-watcher-actor/data-model.md` — entities, fact families, lifecycle.
- `specs/002-git-watcher-actor/contracts/bus-messages.md` — wire contract.
- `specs/002-git-watcher-actor/contracts/cli-surfaces.md` — CLI + structured output.
