# Quickstart — Slice 004 (Buffer Edit)

End-to-end walkthrough exercising US1 / US2 / US3 + verification of SC-401..406. Operator-runnable; the steps under each heading map to one or more e2e tests in `tests/e2e/buffer_edit_*.rs`.

This slice extends slice 003's quickstart by one process: each scenario invokes `weaver edit` or `weaver edit-json` after the bus + service + TUI/subscriber are running. The fifth-process invocation is short-lived (dispatch and exit); the first four processes run for the duration of the scenario.

## Prereqs

- Built workspace at `0.4.0` bus-protocol (post-slice-004 `cargo build --workspace --release`).
- All four binaries on `$PATH`: `weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`.
- A scratch directory for the buffer file: `mkdir -p /tmp/weaver-slice-004 && cd /tmp/weaver-slice-004`.
- Bus socket location convention: `${XDG_RUNTIME_DIR:-/tmp}/weaver.sock` (slice-002+).

## Scenario 1 — Single edit (US1, SC-401, SC-405)

**Setup** (3 terminals or 4 if you want a separate TUI):

```sh
# Terminal A: core
weaver run

# Terminal B: buffer service watching one file
echo "initial content" > /tmp/weaver-slice-004/file.txt
weaver-buffers /tmp/weaver-slice-004/file.txt

# Terminal C: TUI (or subscribe via weaver inspect repeatedly)
weaver-tui
```

Wait for the TUI's Buffers section to show `file.txt` with `version=0` and `dirty=false`.

**Action — single insert at start**:

```sh
# Terminal D
weaver edit /tmp/weaver-slice-004/file.txt 0:0-0:0 "PREFIX "
echo "exit code: $?"   # expect 0
```

**Verify**:

- The TUI's Buffers section flips within ≤500 ms (SC-401 — interactive latency budget):
  - `version` advances `0 → 1`.
  - `byte-size` advances `+7` bytes (length of `"PREFIX "`).
  - `dirty=true` (memory now differs from disk).
- The on-disk file is unchanged: `cat /tmp/weaver-slice-004/file.txt` still prints `initial content`.
- `weaver inspect --why <entity>:buffer/version --output=json` walks back to the accepted `BufferEdit` event (SC-405). The event's provenance renders `{"source":{"type":"user"},...}`.

**Verify the dirty flip semantics** (FR-009):

```sh
# Confirm the TUI's "by service weaver-buffers (inst <UUID>)" attribution
# Confirm the dirty=true badge replaces the previous clean state
```

**E2e coverage**: `tests/e2e/buffer_edit_single.rs` (SC-401 + SC-405 in one fixture).

## Scenario 2 — Atomic batch (US2, SC-402)

**Setup**: continuing from Scenario 1, leave `weaver-buffers` running (now at `version=1`).

**Action — three-edit atomic batch (happy path)**:

```sh
weaver edit /tmp/weaver-slice-004/file.txt \
  0:0-0:7 "" \
  1:0-1:0 "MIDDLE\n" \
  0:0-0:0 "NEW PREFIX "
echo "exit code: $?"   # expect 0
```

(The first edit deletes `"PREFIX "` from line 0; the second inserts a new line; the third inserts a new prefix at the very start. Descending-offset application means edits land later-positions-first, so position-shifting from earlier insertions doesn't invalidate later positions.)

**Verify**:

- `version` advances `1 → 2` exactly once. (Subscribers MUST NOT observe an intermediate `version=1` followed by `version=2`; the bump is a single fact-re-emission.)
- One re-emission burst observable on the subscriber: `byte-size` updated, `version=2` asserted, `dirty=true` re-asserted, all sharing one `causal_parent` (the `BufferEdit` event ID).

**Action — three-edit atomic batch (validation-failure path)**:

```sh
# Build a batch where the middle edit is out-of-bounds (line 9999 doesn't exist)
weaver edit /tmp/weaver-slice-004/file.txt \
  0:0-0:0 "OK1 " \
  9999:0-9999:0 "OUT-OF-BOUNDS" \
  0:0-0:0 "OK2 "
echo "exit code: $?"   # expect 0 (CLI dispatched successfully; service rejected silently)
```

**Verify**:

- `version` stays at `2` (no bump despite CLI exit 0).
- No `buffer/*` fact is re-emitted.
- The on-disk file is unchanged.
- The service's stderr (run with `RUST_LOG=weaver_buffers=debug`) shows a `tracing::debug` line with `reason="validation-failure-out-of-bounds"`, `event_id=<X>`, `entity=<E>`, `edit_index=1` (zero-based).

**E2e coverage**: `tests/e2e/buffer_edit_atomic_batch.rs` (both paths in one fixture).

## Scenario 3 — Sequential 100 edits (US1, SC-403)

**Setup**: fresh buffer.

```sh
echo "" > /tmp/weaver-slice-004/seq.txt
weaver-buffers /tmp/weaver-slice-004/seq.txt &
WB=$!
sleep 0.5  # let bootstrap land; in tests use a subscribe-and-wait pattern
```

**Action**:

```sh
for i in $(seq 1 100); do
  weaver edit /tmp/weaver-slice-004/seq.txt 0:0-0:0 "."
done
```

**Verify**:

- After completion, `weaver inspect <entity>:buffer/version --output=json` reports `version=100`.
- No gaps observed by a long-lived subscriber: `version` strictly increases by 1 per edit; no `version=4` followed by `version=6` skips. (Subscribers may collapse adjacent updates if they're slow to read, but the trace MUST contain every assertion.)
- Total wall-clock is hardware-dependent (typically ~10–20 s for 100 sequential `weaver edit` invocations dominated by process-spawn cost). **No wall-clock budget is asserted** per spec Q4 — observed time is reported informationally.
- **Cleanup**: `kill $WB`.

**E2e coverage**: `tests/e2e/buffer_edit_sequential.rs` (SC-403 — structural-only).

## Scenario 4 — Stale-version drop (US1 #2, SC-404)

**Setup**: simulated concurrent emitters by running two `weaver edit` invocations near-simultaneously.

```sh
echo "abc" > /tmp/weaver-slice-004/race.txt
weaver-buffers /tmp/weaver-slice-004/race.txt &
WB=$!
sleep 0.5
```

**Action — race two emitters**:

```sh
# Both invocations look up version=0 in parallel (a race window of microseconds)
weaver edit /tmp/weaver-slice-004/race.txt 0:0-0:0 "FAST " &
weaver edit /tmp/weaver-slice-004/race.txt 0:0-0:0 "SLOW " &
wait
```

In a real race window only ONE will land; the other will arrive at the service with stale `version=0` (because the first landed and bumped to `version=1`).

**Verify**:

- Both CLI invocations exit `0` (fire-and-forget; CLI cannot detect stale-drop).
- `version=1` exactly (not `2`) — confirming one-of-two won the race; one was silently dropped.
- File content reflects only ONE prefix (either `"FAST abc"` or `"SLOW abc"`, depending on which won the race).
- Service stderr (`RUST_LOG=weaver_buffers=debug`) shows one `tracing::debug` line with `reason="stale-version"`, `emitted_version=0`, `current_version=1`.
- **Cleanup**: `kill $WB`.

**Note on race determinism**: the race window is ~milliseconds; in CI the test harness uses a synchronisation primitive (a barrier waiting for both inspect-lookups to complete before both dispatches fire) to guarantee the race occurs deterministically. Tests do NOT rely on parallel-shell timing.

**E2e coverage**: `tests/e2e/buffer_edit_stale_drop.rs` (SC-404).

## Scenario 5 — Buffer not opened (US1 #4)

**Setup**: core running, but NO `weaver-buffers` instance covering the path.

```sh
# Only core is running this scenario
weaver run &
CORE=$!
sleep 0.5
```

**Action**:

```sh
weaver edit /tmp/weaver-slice-004/never-opened.txt 0:0-0:0 "noop"
echo "exit code: $?"   # expect 1
```

**Verify**:

- Exit code `1`.
- Stderr renders `WEAVER-EDIT-001 — buffer not opened: /tmp/weaver-slice-004/never-opened.txt — no fact (entity:<derived-u64>, attribute:buffer/version) is asserted by any authority. Run weaver-buffers <path> to open the buffer.`
- No event was dispatched on the bus (verifiable by inspecting trace post-action).
- **Cleanup**: `kill $CORE`.

**E2e coverage**: folded into `tests/e2e/buffer_edit_single.rs` as a negative-path test fn.

## Scenario 6 — JSON input parity (US3, SC-406)

**Setup**: fresh buffer, slice-003 service.

```sh
echo "data" > /tmp/weaver-slice-004/json.txt
weaver-buffers /tmp/weaver-slice-004/json.txt &
WB=$!
sleep 0.5
```

**Action — same edit via positional and JSON forms**:

```sh
# Positional form
weaver edit /tmp/weaver-slice-004/json.txt 0:0-0:0 "P1 "
# At this point version=1

# Equivalent JSON form against version=1
echo '[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"new-text":"P2 "}]' \
  | weaver edit-json /tmp/weaver-slice-004/json.txt --from -
# At this point version=2
```

**Verify**:

- Both invocations exit `0`.
- Final file in-memory content (rendered by the TUI) is `"P2 P1 data"`.
- Final `buffer/version=2`, `buffer/dirty=true`.

**SC-406 invariant** (asserted by property test, not interactively):

- For any randomly-generated `Vec<TextEdit>` that is wire-equivalent under both emitter paths, the **dispatched bus message bytes** are byte-identical between `weaver edit <PATH> <pairs>` and `weaver edit-json <PATH> --from -`. The property test in `tests/e2e/buffer_edit_emitter_parity.rs` runs both CLIs under a `proptest` harness with 1024 generated batches and asserts byte equality.

**Cleanup**: `kill $WB`.

**E2e coverage**: `tests/e2e/buffer_edit_emitter_parity.rs` (proptest, SC-406).

## Operator-involvement points (STOP-AND-SURFACE)

Per the project's PR-discipline (one logical change per commit; every commit `scripts/ci.sh` green; operator validates wall-clock and TUI-visual checks):

- **SC-401** (single-edit ≤500 ms wall-clock): hardware-dependent. Tests measure and report observed timing; operator judges pass/fail against the 500 ms budget. Surface the observed timing in commit message / PR body.
- **TUI Buffers section visual** when slice 004 lands (no new render regions per `cli-surfaces.md`, but the operator should eyeball that `version` and `dirty=true` flip correctly in the existing rendering).
- **Quickstart manual walkthrough**: operator runs Scenarios 1–6 end-to-end before PR. Drift becomes follow-up commits.

## References

- `specs/004-buffer-edit/spec.md` — user stories US1/US2/US3, success criteria SC-401..406.
- `specs/004-buffer-edit/data-model.md` — `TextEdit`/`Range`/`Position` shapes.
- `specs/004-buffer-edit/contracts/bus-messages.md` — wire shapes used in this walkthrough.
- `specs/004-buffer-edit/contracts/cli-surfaces.md` — `weaver edit` / `weaver edit-json` grammars.
- `specs/003-buffer-service/quickstart.md` — prior quickstart that this extends.
