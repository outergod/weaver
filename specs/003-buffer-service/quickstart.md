# Quickstart — Slice 003 (Buffer Service)

End-to-end walkthrough of the four-process scenario introduced by slice 003:
`weaver` (core) + `weaver-tui` + `weaver-git-watcher` (from slice 002, still running) + `weaver-buffers` (new this slice).
Each step corresponds to a success criterion (SC-301..SC-307) from `spec.md` and exercises the invariants in `data-model.md`.

Target audience: an operator or reviewer verifying the slice by hand, plus the e2e test suite that codifies the same walk.

## Prerequisites

- Rust toolchain per `rust-toolchain.toml` (pinned to 1.94.0 via `fenix`).
- `cargo build --workspace` completes green from repo root.
- `scripts/ci.sh` passes (clippy + fmt-check + test).
- Either a Nix dev shell (`nix develop`) or equivalent host toolchain.

## Build

```bash
cargo build --workspace --bin weaver --bin weaver-tui --bin weaver-git-watcher --bin weaver-buffers
```

All four binaries land in `target/debug/`. Verify:

```bash
./target/debug/weaver --version
./target/debug/weaver-tui --version
./target/debug/weaver-git-watcher --version
./target/debug/weaver-buffers --version
```

Every `--version` output MUST report `bus_protocol: "0.3.0"`. If any reports `0.2.0`, the build was not rebuilt end-to-end.

## Prepare a fixture

```bash
FIXTURE="$(mktemp -d)/slice-003-fixture.txt"
echo "hello buffer" > "$FIXTURE"
ls -la "$FIXTURE"
# verify: regular file, readable, ~13 bytes
```

## SC-301 — Cold start (single buffer)

**Goal**: from a cold start, opening one file should render its state in the TUI within 1 s.

**Three terminal windows.**

**Window 1 — core**:

```bash
./target/debug/weaver run
# Expected: "weaver core started; bus protocol v0.3.0; listening at /run/user/<uid>/weaver.sock"
```

**Window 2 — TUI**:

```bash
./target/debug/weaver-tui
# Expected: Connection: ready (bus v0.3.0, core 0.3.0+<sha>)
# Buffers: (none)
# Repositories: (none)
```

**Window 3 — buffer service**:

```bash
./target/debug/weaver-buffers "$FIXTURE"
# Expected startup log:
#   weaver-buffers 0.1.0 starting
#     paths:
#       /tmp/.../slice-003-fixture.txt
#     socket:   /run/user/<uid>/weaver.sock
#     poll:     250ms
#     instance: <some UUID>
#   [INFO] connected to core (bus protocol v0.3.0)
#   [INFO] published initial state:
#            /tmp/.../slice-003-fixture.txt  [13 bytes]  clean
#            watcher/status = ready
```

**Expected TUI state** (within 1 s):

```
Buffers:
  /tmp/.../slice-003-fixture.txt  [13 bytes]  clean
    by service weaver-buffers (inst <short-uuid>), event 0, 0.XXXs ago
```

**PASS CRITERION (SC-301)**: the Buffers section shows the fixture's path, byte count, and `clean` badge within 1 s of the buffer service's process start.

## SC-302 — External mutation flips dirty

**Goal**: editing the fixture file outside Weaver flips `buffer/dirty` to `true` within 500 ms.

**In any shell** (leave the three windows above running):

```bash
echo "mutated" >> "$FIXTURE"
```

**Expected TUI state** (within 500 ms):

```
Buffers:
  /tmp/.../slice-003-fixture.txt  [13 bytes]  dirty
    by service weaver-buffers (inst <short-uuid>), event <N>, 0.XXXs ago
```

Note: `buffer/byte-size` stays `13` (memory still holds the pre-mutation content — slice 003 does NOT re-read disk into memory on mutation; slice 004+ will). The dirty flag flips because memory digest ≠ disk digest.

**PASS CRITERION (SC-302)**: `dirty` badge shows within 500 ms of the `echo` command.

**Revert** (to exercise dirty → clean return):

```bash
echo "hello buffer" > "$FIXTURE"
```

Dirty should return to `clean` within 500 ms when memory digest once again equals disk digest.

## SC-303 — SIGKILL retracts all facts

**Goal**: forcibly killing the buffer service should retract all its facts within 5 s.

Identify the buffer service process and kill it:

```bash
pkill -9 -f 'weaver-buffers slice-003-fixture.txt'
```

**Expected TUI state** (within 5 s):

```
Buffers: (none)
```

The core's `release_connection` path (slice 002 F1) retracts every `buffer/*` and `watcher/status` fact the dropped connection owned. The TUI renders the empty Buffers section.

**PASS CRITERION (SC-303)**: Buffers section empties within 5 s.

## SC-304 — Two instances, overlapping paths

**Goal**: launching a second buffer service instance on an already-claimed path causes it to exit code 3 within 1 s; the first continues uninterrupted.

**Restart** the buffer service from the SC-301 setup (Window 3):

```bash
./target/debug/weaver-buffers "$FIXTURE"
```

Wait for `watcher/status = ready`. Then in **Window 4** (or a new shell):

```bash
./target/debug/weaver-buffers "$FIXTURE"
# Expected stderr:
#   Error: buffer/* fact family for /tmp/.../slice-003-fixture.txt
#          is already claimed by weaver-buffers instance <first-UUID> (started 0:0X:XX ago).
#     help: only one weaver-buffers instance may own a given buffer entity at a time.
#           Stop the other instance, or open a different file.
#     code: WEAVER-BUF-004
#
echo $?
# Expected: 3
```

The first instance MUST continue serving uninterrupted — its `weaver-tui` state should NOT change.

**PASS CRITERION (SC-304)**: second instance exits code 3 within 1 s; first instance's TUI rendering is unchanged throughout.

## SC-305 — Inspection renders service attribution

**Goal**: inspecting any buffer fact should render `weaver-buffers` (not `core/dirty-tracking`) as the authoring actor.

With the first buffer service still running, in a new shell:

```bash
# Determine the entity id via TUI inspection or `weaver status`.
./target/debug/weaver status --output=json | jq '.facts[] | select(.fact.attribute == "buffer/dirty")'
```

Use the `entity` value from the output:

```bash
./target/debug/weaver inspect <entity>:buffer/dirty
# Expected human output:
#   fact:       <entity>:buffer/dirty
#   source:     service weaver-buffers (instance <UUID>)
#   event:      <poll-tick EventId>
#   asserted:   <timestamp>
#   trace seq:  <N>
```

JSON form:

```bash
./target/debug/weaver inspect <entity>:buffer/dirty --output=json
# Expected: asserting_service: "weaver-buffers", asserting_instance: "<UUID>",
#          asserting_behavior field absent.
```

**PASS CRITERION (SC-305)**: human rendering says `service weaver-buffers (instance <UUID>)`; JSON rendering has `asserting_service` = `"weaver-buffers"` and no `asserting_behavior` field.

**Additionally** (FR-013 — F23 live-fact-provenance check in isolation): the e2e test `buffer_inspect_overwrites_behavior.rs` does this as a unit: injects a behavior-authored `buffer/dirty=true` into the trace, then has the service assert `buffer/dirty=false` on the same key, then asserts inspect returns the service attribution. Verified programmatically, not manually.

## SC-306 — Component-discipline property

**Goal**: no fact value produced by the buffer service carries buffer content.

This is a property test, not a manual walkthrough step. The test (`buffers/tests/component_discipline.rs`) runs under `cargo test` and asserts:

- For any random observation sequence over any random content, every `Fact` the publisher emits satisfies `matches!(fact.value, FactValue::String(_) | FactValue::U64(_) | FactValue::Bool(_))`.
- No `FactValue::Bytes`, no `FactValue::String` containing the file's content, no other variant that could smuggle content.

Run locally:

```bash
cargo test -p weaver-buffers component_discipline
```

**PASS CRITERION (SC-306)**: property test passes with 1000+ proptest iterations.

## SC-307 — Slice-001 e2e tests transformed, not dropped

**Goal**: the `hello_fact` and `disconnect` e2e tests continue to pass, rewritten to drive `buffer/open` + external-mutation instead of `simulate-edit` / `simulate-clean`.

```bash
cargo test -p weaver-e2e --test hello_fact
cargo test -p weaver-e2e --test disconnect
```

Both suites green. The test bodies reference `weaver-buffers` (not `weaver simulate-edit`) and create fixture files via `tempfile::TempDir` as described in `research.md §5`.

**PASS CRITERION (SC-307)**: both tests green end-to-end with bus protocol 0.3.0.

## Migration (notes for developers running slice-001 era scripts)

Any script or tool that previously invoked:

```bash
weaver simulate-edit 1        # slice 001
weaver simulate-clean 1       # slice 001
```

will now fail with a clap parse error (exit code 2). The slice-003-equivalent invocation is:

```bash
# 1. Create a real file.
FIXTURE="$(mktemp)"
echo "initial" > "$FIXTURE"

# 2. Open it with the buffer service.
weaver-buffers "$FIXTURE" &

# 3. To trigger dirty:
echo "mutated" >> "$FIXTURE"

# 4. To trigger clean (revert memory → disk):
#    slice 003 has no in-process mutation, so the "clean" transition
#    requires the on-disk content to match the in-memory content.
#    The cleanest path is: re-open the file (killing + restarting
#    the service re-reads disk into memory, yielding clean).
```

Slice 004 introduces a `buffer/save` event that flushes memory to disk, at which point the clean transition becomes a first-class operation.

## Failure-mode walkthroughs

Each failure mode from `contracts/bus-messages.md` §Failure modes has a matching e2e test:

| Failure mode | e2e test | Observable outcome |
|---|---|---|
| Path missing at startup | `buffer_startup_failure.rs` (part of `buffer_open_bootstrap.rs`) | Service exits 1 with `WEAVER-BUF-001` diagnostic; no facts published |
| Path is a directory | same | Service exits 1 with `WEAVER-BUF-002` |
| File deleted mid-session | `buffer_external_mutation.rs` (variant) | `buffer/observable=false` asserted once; other buffers unaffected; `watcher/status` stays `ready` |
| All buffers unobservable simultaneously | `buffer_observable.rs` | `watcher/status=degraded` asserted once; recovery re-asserts `ready` |
| Two instances same path | `buffer_authority_conflict.rs` | Second exits 3 within 1 s; first unaffected |
| SIGKILL of service | `buffer_sigkill.rs` | All facts retracted within 5 s; Buffers section empties |
| Core restart while service connected | `buffer_core_eof.rs` | Service exits with code 2 (bus unavailable), no reconnect |

## Expected trace shape

A successful `weaver-buffers <file>` bootstrap produces the following trace entries (shown as authoritative entries only; retractions and other noise elided):

```
seq  kind               fact                                     source                      causal_parent
---  -----------------  ---------------------------------------  --------------------------  -------------
1    FactAsserted       <instance>:watcher/status = "started"   service weaver-buffers      None
2    FactAsserted       <buffer>:buffer/path = "/tmp/.../file"  service weaver-buffers      Some(E₁)
3    FactAsserted       <buffer>:buffer/byte-size = 13          service weaver-buffers      Some(E₁)
4    FactAsserted       <buffer>:buffer/dirty = false           service weaver-buffers      Some(E₁)
5    FactAsserted       <buffer>:buffer/observable = true       service weaver-buffers      Some(E₁)
6    FactAsserted       <instance>:watcher/status = "ready"     service weaver-buffers      None
```

`E₁` is the synthesized bootstrap-tick `EventId` for this buffer. With N buffers, entries 2–5 repeat N times with per-buffer `E₁`, `E₂`, … `Eₙ`; entry 6 fires once after all N bootstrap.

## References

- `specs/003-buffer-service/spec.md` — user stories, success criteria, clarifications.
- `specs/003-buffer-service/plan.md` — technical context, constitution check.
- `specs/003-buffer-service/data-model.md` — internal types, lifecycle state machines, validation rules.
- `specs/003-buffer-service/contracts/bus-messages.md` — wire contract delta.
- `specs/003-buffer-service/contracts/cli-surfaces.md` — CLI surfaces for all four binaries.
- `specs/003-buffer-service/research.md` — library choices, observation strategy.
- `specs/002-git-watcher-actor/quickstart.md` — three-process walkthrough this quickstart extends to four.
