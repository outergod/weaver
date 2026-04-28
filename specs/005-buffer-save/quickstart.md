# Quickstart — Slice 005 (Buffer Save)

End-to-end walkthrough exercising US1 / US2 / US3 + verification of SC-501..507. Operator-runnable; the steps under each heading map to one or more e2e tests in `tests/e2e/buffer_save_*.rs` and `tests/e2e/multi_producer_stamping.rs` / `tests/e2e/event_outbound_codec_validation.rs`.

This slice extends slice 004's five-process quickstart by adding `weaver save` as a sixth-process invocation. Each scenario presupposes the slice-004 setup (core + git-watcher + buffers + TUI/subscriber + `weaver edit`) and adds the save step.

## Prereqs

- Built workspace at `0.5.0` bus-protocol (post-slice-005 `cargo build --workspace --release`).
- All four binaries on `$PATH`: `weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`.
- A scratch directory for the buffer file: `mkdir -p /tmp/weaver-slice-005 && cd /tmp/weaver-slice-005`.
- Bus socket location convention: `${XDG_RUNTIME_DIR:-/tmp}/weaver.sock` (slice-002+).

## Scenario 1 — Save an edited buffer (US1, SC-501)

**Setup**:

```sh
# Terminal A: core
weaver run

# Terminal B: buffer service watching one file
echo "initial content" > /tmp/weaver-slice-005/file.txt
weaver-buffers /tmp/weaver-slice-005/file.txt

# Terminal C: TUI
weaver-tui
```

Wait for the TUI's Buffers section to show one row per opened file. Bootstrap-time per-row format (inherited from slice 004):

```
  /tmp/weaver-slice-005/file.txt  [v=0] [16 bytes] clean
    by service weaver-buffers (inst <short-uuid>), event EventId(<n>), <t>s ago
```

**Action — edit then save**:

```sh
# Terminal D
weaver edit /tmp/weaver-slice-005/file.txt 0:0-0:0 "PREFIX "
# TUI flips: [v=0] → [v=1], [16 bytes] → [23 bytes], clean → dirty (SC-401 from slice 004; precondition)

# Pause briefly to allow the dirty state to render in the TUI before saving
weaver save /tmp/weaver-slice-005/file.txt
echo "exit code: $?"   # expect 0
```

**Verify** (the TUI Buffers row flips within ≤500 ms — SC-501 interactive latency budget):

- `dirty → clean` — `buffer/dirty` flips back to `false`. Operator-visible state-flip signal.
- `[v=1]` unchanged — save does NOT bump `buffer/version` (FR-004).
- `[23 bytes]` unchanged — `buffer/byte-size` is not re-emitted on save (FR-004).
- The annotation line refreshes: `event EventId(<n>)` now carries the BufferSave event's stamped id (different from the BufferEdit id from the prior step).
- **The on-disk file matches the in-memory content**: `cat /tmp/weaver-slice-005/file.txt` prints `PREFIX initial content` (the post-edit, post-save content).
- `weaver inspect --why <entity>:buffer/dirty --output=json` walks back to the accepted `BufferSave` event. The event's provenance renders `{"source":{"type":"user"},...}` — `ActorIdentity::User`.

**Verify the §28(a) stamping invariant** (FR-021):

- The `event EventId(<n>)` field renders a small monotonic integer (e.g., `EventId(4)` after a few prior events), NOT a `now_ns()`-shaped value (~`1714217040123456789`). The stamped IDs are listener-allocated counters starting from `1`.
- A second observer subscribed to the same trace sees the SAME EventId for the BufferSave — the listener's allocation is the canonical identity.

**E2e coverage**: `tests/e2e/buffer_save_dirty.rs` (SC-501 + §28(a) stamping verification in one fixture).

## Scenario 2 — Refuse save on inode mismatch (US2, SC-502)

**Setup**: continuing from Scenario 1; leave `weaver-buffers` running. The buffer is currently clean at `version=1` after the save.

**Action — induce another edit, then externally rename the file, then attempt to save**:

```sh
# Terminal D
weaver edit /tmp/weaver-slice-005/file.txt 0:0-0:0 "MORE "
# TUI flips back to dirty: [v=1] → [v=2], [23 bytes] → [28 bytes], clean → dirty

# Externally rename the file (OUTSIDE Weaver; simulating git checkout, mv, etc.)
mv /tmp/weaver-slice-005/file.txt /tmp/weaver-slice-005/file.txt.bak

# Now attempt to save
weaver save /tmp/weaver-slice-005/file.txt
echo "exit code: $?"   # expect 0 (CLI doesn't know the save will be refused)
```

**Verify**:

- The TUI Buffers row does NOT flip back to `clean`. `dirty` persists; `[v=2]` persists.
- `weaver-buffers`'s stderr emits a `WEAVER-SAVE-005` warn record:
  ```
  WARN weaver_buffers: WEAVER-SAVE-005 path/inode mismatch on save
    entity=<entity-ref> path=/tmp/weaver-slice-005/file.txt
    expected_inode=<inode-at-open> actual_inode=<missing>
    event_id=<stamped-id>
  ```
  (Or for the inode-delta case, `actual_inode=<some-other-value>` instead of `<missing>`.)
- `cat /tmp/weaver-slice-005/file.txt.bak` still prints `initial content` (the pre-edit content; the rename happened externally before the save attempt; the buffer's edits never made it to disk).
- Nothing exists at `/tmp/weaver-slice-005/file.txt`: `ls /tmp/weaver-slice-005/` shows `file.txt.bak` only.

**Recovery posture**: the operator may `mv /tmp/weaver-slice-005/file.txt.bak /tmp/weaver-slice-005/file.txt` to restore the path, then re-run `weaver save`. Note: this restores the path but does NOT re-establish the inode equality (the inode was captured at `BufferOpen` time and won't match the restored file's new inode unless the filesystem happens to reuse the freed inode number). For the slice-005 MVP, the operator's correct recovery is to **re-open the buffer** (kill `weaver-buffers`, re-launch with the path) — the new `BufferOpen` will capture a fresh inode and subsequent saves will succeed.

**E2e coverage**: `tests/e2e/buffer_save_inode_refusal.rs` (SC-502).

## Scenario 3 — Refuse save on path deletion (US2, SC-503)

**Setup**: re-establish a clean buffer state. Restart `weaver-buffers` if necessary to capture a fresh inode (per the recovery posture in Scenario 2).

```sh
# Re-open
echo "fresh content" > /tmp/weaver-slice-005/file.txt
# (kill and restart weaver-buffers if Scenario 2 left it in inode-mismatch state)
weaver-buffers /tmp/weaver-slice-005/file.txt &
```

**Action — edit, then externally delete the file, then attempt to save**:

```sh
weaver edit /tmp/weaver-slice-005/file.txt 0:0-0:0 "EDITED "
# Buffer is now dirty at v=1.

rm /tmp/weaver-slice-005/file.txt

weaver save /tmp/weaver-slice-005/file.txt
echo "exit code: $?"   # expect 0
```

**Verify**:

- The TUI Buffers row does NOT flip to `clean`. Dirty persists.
- `weaver-buffers`'s stderr emits a `WEAVER-SAVE-006` warn record:
  ```
  WARN weaver_buffers: WEAVER-SAVE-006 path missing on save
    entity=<entity-ref> path=/tmp/weaver-slice-005/file.txt
    event_id=<stamped-id>
  ```
- `ls /tmp/weaver-slice-005/file.txt` reports `No such file or directory` — the save did NOT recreate the file.
- The buffer's in-memory state is unchanged; the operator can re-create the file externally + restart `weaver-buffers` to recover.

**E2e coverage**: `tests/e2e/buffer_save_inode_refusal.rs` (SC-503; same test file as SC-502 — both refusal modes).

## Scenario 4 — Atomic-rename invariant under I/O failure (SC-504)

**Setup**: this scenario uses an in-process test harness rather than a six-terminal walk; it is included here for completeness of the spec's verification surface, but operator-runnable verification requires a custom build of the test binary or manual filesystem manipulation.

**Action** (test-binary-driven; see `tests/e2e/buffer_save_atomic_invariant.rs`):

The test:
1. Sets up a `weaver-buffers`-equivalent with a buffer at `version=1`, dirty.
2. Calls `BufferState::save_to_disk` with a hook closure that returns `Err(io::Error::new(ErrorKind::OutOfStorage, "ENOSPC"))` on `WriteStep::RenameToTarget`.
3. Asserts:
   - The on-disk file is byte-identical to its pre-save state (atomic-rename invariant — under any failure between tempfile write and rename, the original is preserved).
   - The tempfile has been cleaned up (`std::fs::read_dir(parent)` shows no `.weaver-save.<uuid>` orphans).
   - The save outcome is `BufferSaveOutcome::RenameIo { error, .. }` and the dispatcher emitted `WEAVER-SAVE-004` at `error` level.

**Verify** (test passes):

- `cargo test --test buffer_save_atomic_invariant` exits `0`.
- `WEAVER-SAVE-004` is captured in the test's tracing output.

**E2e coverage**: `tests/e2e/buffer_save_atomic_invariant.rs` (SC-504).

## Scenario 5 — Multi-producer EventId stamping (US3, SC-505)

**Setup**: this scenario also uses an in-process test harness for stress-volume control; manual reproduction would require launching three concurrent producer scripts and is impractical for routine quickstart use.

**Action** (test-binary-driven; see `tests/e2e/multi_producer_stamping.rs`):

The test:
1. Sets up a core listener + trace store.
2. Spawns three producers in parallel:
   - Producer A: emits 1000 `BufferEdit` events on a buffer entity.
   - Producer B: emits 1000 `BufferSave` events on the same entity.
   - Producer C: emits 1000 git-watcher poll-tick events.
3. After all producers finish, walks every accepted event's `causal_parent` chain via `weaver inspect --why` (in-process invocation).
4. Asserts: every walkback resolves to the correct source producer (verified by cross-checking `event.provenance.source` against the producer that originated the event).

**Verify** (test passes):

- `cargo test --test multi_producer_stamping` exits `0`.
- 100% of walkbacks resolve correctly. Pre-§28(a) (slice 004 baseline) would have non-zero collision rate under this stress; post-§28(a) the rate is structurally zero.

**E2e coverage**: `tests/e2e/multi_producer_stamping.rs` (SC-505).

## Scenario 6 — Producer-supplied non-Outbound event rejection (US3, SC-506)

**Setup**: also test-binary-driven; the scenario exercises a wire-shape rejection that operator tooling will not normally encounter (post-§28(a) clients always construct `EventOutbound` correctly).

**Action** (test-binary-driven; see `tests/e2e/event_outbound_codec_validation.rs`):

The test:
1. Sets up a core listener.
2. Establishes a bus connection.
3. Manually serialises a `BusMessageInbound::Event(_)` frame whose payload is the wire shape of an `Event { id, .. }` (with an `id` field — i.e., a slice-004-or-earlier client's outbound shape).
4. Sends the frame.
5. Asserts: the codec returns a structured decode error to the producer; the connection receives `BusMessage::Error { category: "decode", .. }` and closes; the trace contains no entry for the rejected event.

**Verify**:

- `cargo test --test event_outbound_codec_validation` exits `0`.
- `BusMessage::Error` with `category: "decode"` observed on the producer's connection.

**E2e coverage**: `tests/e2e/event_outbound_codec_validation.rs` (SC-506).

## Scenario 7 — Save against clean buffer (SC-507)

**Setup**: continuing from Scenario 1's clean post-save state. The buffer is at `version=1`, `dirty=false`.

**Action — save the already-clean buffer**:

```sh
# Terminal D
sleep 1   # allow at least 1 second to elapse so we can detect mtime preservation
ls -la --time=ctime /tmp/weaver-slice-005/file.txt > /tmp/before-mtime.txt

weaver save /tmp/weaver-slice-005/file.txt
echo "exit code: $?"   # expect 0

ls -la --time=ctime /tmp/weaver-slice-005/file.txt > /tmp/after-mtime.txt
diff /tmp/before-mtime.txt /tmp/after-mtime.txt
echo "diff exit: $?"   # expect 0 (no diff — mtime preserved)
```

**Verify**:

- `weaver-buffers`'s stderr emits a `WEAVER-SAVE-007` info record:
  ```
  INFO weaver_buffers: WEAVER-SAVE-007 nothing to save: buffer was already clean
    entity=<entity-ref> path=/tmp/weaver-slice-005/file.txt
    event_id=<stamped-id> version=1
  ```
- The TUI Buffers row remains in `clean` state. The annotation line refreshes — `event EventId(<n>)` now points at the latest BufferSave event (different from Scenario 1's id) — but the value `clean` is unchanged.
- `diff /tmp/before-mtime.txt /tmp/after-mtime.txt` exits `0` — the file's mtime was NOT touched. SC-507 verifies no disk I/O on clean save.
- `weaver inspect --why <entity>:buffer/dirty` walkback resolves to this latest BufferSave event (the most-recent re-assertion of `dirty=false`). The walkback IS to the no-op save, not to the original Scenario 1 save — this is the slice-004 precedent (FR-009 re-emits on every accepted operation).

**E2e coverage**: `tests/e2e/buffer_save_clean_noop.rs` (SC-507).

## Verification checklist

After completing scenarios 1–7, the operator has demonstrated:

- [x] **SC-501**: edit → save → `buffer/dirty = false` within interactive latency; on-disk content matches in-memory.
- [x] **SC-502**: external rename between open and save → `WEAVER-SAVE-005` + buffer state preserved + original file content preserved.
- [x] **SC-503**: external delete between open and save → `WEAVER-SAVE-006` + no file created + buffer state preserved.
- [x] **SC-504**: I/O failure between tempfile write and rename → original file byte-identical to pre-save state + tempfile cleaned up + `WEAVER-SAVE-004` (test-binary verification).
- [x] **SC-505**: multi-producer stress → 100% `weaver inspect --why` walkback resolution (test-binary verification).
- [x] **SC-506**: wire-shape rejection on `Event`-with-id from inbound channel (test-binary verification).
- [x] **SC-507**: clean-save no-op → `WEAVER-SAVE-007` + idempotent `buffer/dirty = false` re-emission + mtime preserved.

## Operator-pace notes

- **SC-501 latency**: the ≤500 ms budget is operator-perceived (TUI flip from `dirty` to `clean`). The actual save flow on commodity SSD typically completes in <20 ms; the budget has ~25× margin and is robust to filesystem variation.
- **SC-507 mtime check**: mtime granularity on most filesystems is 1 second (some are nanosecond-precise; a few are 2-second-resolution e.g. FAT). The `sleep 1` before the `before-mtime.txt` capture ensures a measurable mtime baseline; the post-save `after-mtime.txt` should match within filesystem granularity.
- **SC-505 / SC-506 are test-binary-only**: there is no realistic operator workflow that emits 3000 events in tight succession or that constructs a malformed wire frame manually. These scenarios verify implementation correctness, not operator UX.
- **Walkthrough order matters for the mtime check (SC-507)**: Scenario 1 must run before Scenario 7 (Scenario 1's save is what writes the file with the mtime that Scenario 7 then preserves).

## Slice-005 carried-over hygiene

Per FR-025: `core/src/cli/edit.rs::handle_edit_json` grew a code comment at the post-parse step explaining why empty `[]` JSON does NOT short-circuit (asymmetric with positional zero-pair). This is implementation-internal and not directly operator-observable; the comment improves the path's reviewability for future contributors.

---

*Phase 1 — quickstart.md complete. CLAUDE.md SPECKIT-block update follows as the final Phase-1 deliverable.*
