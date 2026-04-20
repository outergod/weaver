# Quickstart: Hello, fact

Build, run, and verify the Hello-fact slice end-to-end. Validates spec Success Criteria SC-001 through SC-006.

## Prerequisites

- Rust stable toolchain (pinned via `rust-toolchain.toml`; `rustup` will install on first build)
- A POSIX-compatible terminal (Linux or macOS)
- `git` (for build-time provenance)

## Build

From the repository root:

```bash
cargo build --workspace
```

First build will fetch dependencies and may take a few minutes. Subsequent builds are incremental.

To build with optimizations:

```bash
cargo build --workspace --release
```

## Run

The slice requires two terminal windows.

### Terminal 1 — start the core

```bash
cargo run --bin weaver -- run
```

You should see structured log output ending in a `Lifecycle::Ready` line. The core is now listening on the bus socket (default `$XDG_RUNTIME_DIR/weaver.sock`).

### Terminal 2 — start the TUI

```bash
cargo run --bin weaver-tui
```

You should see:

```
┌─ Weaver TUI ────────────────────────────────────────────────────┐
│ Connection: ready (bus v0.1.0, core 0.1.0+...)                  │
│                                                                 │
│ Facts: (none)                                                   │
│                                                                 │
│ Commands: [e]dit  [c]lean  [i]nspect  [q]uit                    │
└─────────────────────────────────────────────────────────────────┘
```

## Verify the slice

### SC-001 — happy path: edit → dirty → clean

In the TUI:

1. Press `e` (simulate-edit). Observe the Facts list update within 100 ms to show `buffer/dirty(EntityRef(1)) = true`.
2. Press `c` (simulate-clean). Observe the Facts list return to `(none)` within 100 ms.

**Pass criterion**: both transitions visible without restart, latency below the interactive class threshold.

### SC-002 — provenance inspection

In the TUI:

1. Press `e` to assert the dirty fact.
2. Press `i` (inspect). Observe a non-empty inspection result naming the source event ID, the asserting behavior (`core/dirty-tracking`), and a recent timestamp.

**Pass criterion**: the inspection response contains all three fields, with no placeholder values.

### SC-003 — structured machine output

In a third terminal (with the core still running):

```bash
cargo run --bin weaver -- status --output=json | jq .
```

Then trigger an edit from the TUI (or via `cargo run --bin weaver -- simulate-edit 1`) and re-run the status command. Compare the JSON output before and after — the `facts` array should grow by one entry.

Pipe a single status query to a Rust deserializer test (or `jq` field extraction) and confirm the field names match those in `contracts/cli-surfaces.md`.

**Pass criterion**: the output is valid JSON; the field names mirror the bus vocabulary; deserialization round-trip preserves identity.

### SC-004 — graceful degradation when core stops

With the TUI showing the `dirty` fact:

1. In Terminal 1, hit `Ctrl-C` to stop the core.
2. Within 5 seconds, the TUI's connection indicator changes to `UNAVAILABLE` and the facts area is marked stale.
3. The TUI does not crash. Pressing `q` cleanly exits.

**Pass criterion**: TUI surfaces the loss within 5 seconds; no panic, no fictional state.

### SC-005 — automated test coverage

```bash
cargo test --workspace
```

Expected output: all tests pass. The suite includes:

- Pure helper unit tests (`provenance::Provenance::new` invariants, `EntityRef` ordering, etc.).
- Scenario tests on the dirty-tracking behavior (`(empty fact-space, [BufferEdited]) → asserts buffer/dirty`).
- Scenario tests on retraction (`(dirty fact-space, [BufferCleaned]) → retracts buffer/dirty`).
- Property tests on fact-space invariants (assert/retract round-trip; provenance non-empty; sequence monotonicity).
- An end-to-end test (`tests/e2e/hello_fact.rs`) that spawns the core, connects a test client, exercises the happy path and one failure mode (core unavailable).

**Pass criterion**: zero failures; coverage spans happy + retraction + degradation paths.

### SC-006 — version output

```bash
time cargo run --bin weaver -- --version
```

Expected output (similar):

```
weaver 0.1.0
  commit: a1b2c3d (dirty)
  built:  2026-04-19T15:30:00Z
  profile: debug
  bus protocol: v0.1.0
```

Wall-clock time including process startup should be well under 50 ms (the requirement is on the version-rendering step itself; cargo's startup overhead is excluded by running the built binary directly):

```bash
./target/debug/weaver --version  # or release/weaver
```

**Pass criterion**: all five fields present; build-time provenance reflects the actual git state.

## Manual exploration

After verifying the success criteria, useful exploratory commands:

```bash
# JSON inspection from the CLI (thin wrapper over the bus inspect request)
weaver inspect 1:buffer/dirty --output=json | jq .

# Watch tracing output while running (via RUST_LOG)
RUST_LOG=weaver=debug cargo run --bin weaver -- run

# Confirm the bus socket exists while core is running
ls -la $XDG_RUNTIME_DIR/weaver.sock
```

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `weaver-tui` reports `UNAVAILABLE` immediately | Core not running or wrong socket path | Start core first; check `$WEAVER_SOCKET` |
| `weaver --version` shows `commit: unknown` | `vergen` build script failed (e.g., not in a git checkout) | Re-clone or check `git status` |
| TUI shows stale facts after core restart | Auto-reconnect is out of scope for this slice (deferred to a later milestone per spec Assumptions) | Press `q`, restart the TUI |
| `cargo test` hangs on the e2e test | Test couldn't bind the temporary socket; another core is running | Stop other cores; `lsof $XDG_RUNTIME_DIR/weaver.sock` |
