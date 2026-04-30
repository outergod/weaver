# Quickstart — Slice 005 (Buffer Save)

End-to-end walkthrough exercising US1 / US2 / US3 + verification of SC-501..507. Operator-runnable; the steps under each heading map to one or more e2e tests in `tests/e2e/buffer_save_*.rs` and `tests/e2e/multi_producer_uuidv8.rs` / `tests/e2e/eventid_uuid_strict_parsing.rs`.

This slice extends slice 004's five-process quickstart by adding `weaver save` as a sixth-process invocation. Each scenario presupposes the slice-004 setup (core + git-watcher + buffers + TUI/subscriber + `weaver edit`) and adds the save step.

## Prereqs

- Built workspace at `0.5.0` bus-protocol (post-slice-005 `cargo build --workspace --release`).
- All four binaries on `$PATH`: `weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`.
- `jq` for the entity-id lookup helper below.
- A scratch directory for the buffer file: `mkdir -p /tmp/weaver-slice-005 && cd /tmp/weaver-slice-005`.
- Bus socket location convention: `${XDG_RUNTIME_DIR:-/tmp}/weaver.sock` (slice-002+).

### Helper — look up a buffer's entity-id (u64)

`weaver inspect` and `weaver save`'s u64-form (used in Scenarios 3 below) take a u64 EntityRef rather than a path. Define this helper once at the start of the walkthrough:

```sh
buffer_entity() {
    weaver status -o json | jq -r --arg p "$1" '
        .facts[] | select(.key.attribute=="buffer/path" and .value.value==$p) | .key.entity
    '
}
# Usage: ENTITY=$(buffer_entity /tmp/weaver-slice-005/file.txt)
```

The helper queries the live fact store for the `buffer/path` fact whose value matches the given canonical path and prints the associated entity's u64. `weaver save` accepts both the u64 form and the path form (auto-detect); `weaver inspect`'s fact-key parser accepts ONLY the u64 form (`core/src/cli/inspect.rs::parse_fact_key`).

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
    by service weaver-buffers (inst <short-uuid>), event EventId(<uuid>), <t>s ago
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
- The annotation line refreshes: `event EventId(<n>)` now carries the BufferSave event's UUIDv8 id (different from the BufferEdit id from the prior step).
- **The on-disk file matches the in-memory content**: `cat /tmp/weaver-slice-005/file.txt` prints `PREFIX initial content` (the post-edit, post-save content).
- `ENTITY=$(buffer_entity /tmp/weaver-slice-005/file.txt)`; `weaver inspect $ENTITY:buffer/dirty --why -o json` walks back to the accepted `BufferSave` event. The event's provenance renders `{"source":{"type":"user"},...}` — `ActorIdentity::User`.

**Verify the §28(a) UUIDv8 producer-prefix invariant** (FR-019..FR-022):

- **TUI rendering** (User-emitted events): the `event EventId(<n>)` field on the buffer rows renders as **raw UUID hex** (`EventId(1f7eca88-2045-89c0-98ab-10fa2182c909)`). The TUI's passive-cache populates from observed `FactAssert` provenance only — and in slice 005's flow the only fact-emitters are `weaver-buffers` (Service); `weaver edit` / `weaver save` are fire-and-forget User producers and never assert facts of their own. The User-prefix → "user" cache binding is therefore never observed by the TUI in this slice; "bootstrap miss is acceptable" per the session-2 architectural decision (renders raw UUID until/unless a User-authored fact is observed).
- **`weaver inspect --why` rendering** (human output): the inspect path's per-invocation cache DOES populate from the walked-back event's `provenance.source`. For a User-emitted BufferSave event the human output renders `EventId(user/<short-hex-suffix>)`. JSON output (`-o json`) emits the full UUID hex regardless, for grep-ability.
- The `BufferSave` event's UUID's high 58 bits encode the User-process's hashed UUIDv4 prefix (per `mint_v8`); the low ~62 bits encode the producer's mint-time nanoseconds. The prefix differs from `weaver-buffers`'s Service-prefix on its bootstrap-tick / poll-tick events — distinct producers occupy disjoint prefix namespaces.
- A second observer subscribed to the same trace sees the SAME UUID for the BufferSave — the producer's local mint is the canonical identity (no listener-stamping reconciliation).
- The `weaver inspect --why $ENTITY:buffer/dirty -o json` output's `event.id` field is the full UUID hex (e.g., `"id": "1f7eca88-2045-89c0-98ab-10fa2182c909"`). To verify the structural UUIDv8 layout by hand: byte 6 high nibble must be `8` (version), byte 8 high two bits must be `0b10` (RFC 4122 variant). To extract the 58-bit prefix in Python:

  ```sh
  python3 -c '
  import sys
  v = int.from_bytes(bytes.fromhex(sys.argv[1].replace("-","")), "big")
  custom_a = (v >> 80) & ((1 << 48) - 1)
  custom_b = (v >> 64) & ((1 << 12) - 1)
  prefix = (custom_a << 10) | ((custom_b >> 2) & ((1 << 10) - 1))
  print(f"prefix={prefix:016x}  version={(v >> 76) & 0xf}  variant={(v >> 62) & 3:02b}")
  ' 1f7eca88-2045-89c0-98ab-10fa2182c909
  ```

  All `weaver-buffers`-authored facts on the same buffer (`buffer/dirty`, `buffer/byte-size`, `buffer/path`, `buffer/version`) share the SAME prefix when run through `extract_prefix` — that's the Service-prefix invariant. A User-emitted save event's prefix differs (different producer namespace).

**E2e coverage**: `tests/e2e/buffer_save_dirty.rs` (SC-501).

## Scenario 2 — Refuse save on inode mismatch (US2, SC-502)

**Setup**: continuing from Scenario 1; leave `weaver-buffers` running. The buffer is currently clean at `version=1` after the save.

**Action — induce another edit, then externally atomic-replace the file with a different inode at the same path, then attempt to save**:

```sh
# Terminal D
weaver edit /tmp/weaver-slice-005/file.txt 0:0-0:0 "MORE "
# TUI flips back to dirty: [v=1] → [v=2], [23 bytes] → [28 bytes], clean → dirty

# Externally atomic-replace the file (OUTSIDE Weaver; simulates another editor's
# atomic-save pattern: write to a sibling, then rename(2) over the target).
# rename(2) preserves the source inode, so the new inode at <PATH> differs
# from the inode the buffer captured at BufferOpen time.
cp /tmp/weaver-slice-005/file.txt /tmp/weaver-slice-005/file.txt.bak
echo "externally written" > /tmp/weaver-slice-005/file.txt.new
mv /tmp/weaver-slice-005/file.txt.new /tmp/weaver-slice-005/file.txt

# Now attempt to save (path form works because <PATH> still exists post-replace)
weaver save /tmp/weaver-slice-005/file.txt
echo "exit code: $?"   # expect 0 (CLI doesn't know the save will be refused)
```

**Verify**:

- The TUI Buffers row does NOT flip back to `clean`. `dirty` persists; `[v=2]` persists.
- `weaver-buffers`'s stderr emits a `WEAVER-SAVE-005` warn record (under `RUST_LOG=weaver_buffers=info` or higher):
  ```
  WARN weaver_buffers: WEAVER-SAVE-005 path/inode mismatch on save
    code="WEAVER-SAVE-005" entity=<entity-ref-u64> event_id=<uuidv8>
    path=/tmp/weaver-slice-005/file.txt
    expected_inode=<inode-at-open> actual_inode=<inode-of-replacement>
  ```
  Both `expected_inode` and `actual_inode` are numeric u64s; they differ by construction.
- `cat /tmp/weaver-slice-005/file.txt` prints `externally written` — the no-clobber invariant: the dispatcher refused before any tempfile write hit the target. The externally-written content is preserved byte-for-byte.
- `cat /tmp/weaver-slice-005/file.txt.bak` prints `MORE PREFIX initial content` — the pre-replace snapshot we copied for reference.

**Recovery posture**: the operator must **re-open the buffer** to capture a fresh inode (kill `weaver-buffers`, re-launch with the path). The captured inode is immutable for the buffer's lifetime per FR-006; an externally-replaced path cannot be reconciled without re-opening.

**E2e coverage**: `tests/e2e/buffer_save_inode_refusal.rs::external_atomic_replace_fires_save_005` (SC-502).

## Scenario 3 — Refuse save on path missing (US2, SC-503)

**Setup**: re-establish a clean buffer state. Restart `weaver-buffers` if necessary to capture a fresh inode (per the recovery posture in Scenario 2).

```sh
# Re-open with fresh content
echo "fresh content" > /tmp/weaver-slice-005/file.txt
# (kill and restart weaver-buffers if Scenario 2 left it in inode-mismatch state)
weaver-buffers /tmp/weaver-slice-005/file.txt &

# Capture the entity-id NOW while the path still exists; needed for the u64-form
# save below (after rm, the helper would no longer find the buffer/path fact —
# although weaver-buffers's open-time fact persists, the helper queries by value
# so works either way; capturing here is hygiene).
ENTITY=$(buffer_entity /tmp/weaver-slice-005/file.txt)
echo "entity=$ENTITY"
```

**Action — edit, then externally delete (or rename-away) the file, then attempt to save via u64-form**:

```sh
weaver edit /tmp/weaver-slice-005/file.txt 0:0-0:0 "EDITED "
# Buffer is now dirty at v=1.

rm /tmp/weaver-slice-005/file.txt
# (Equivalently: `mv /tmp/weaver-slice-005/file.txt /tmp/somewhere/else.txt` —
# either way the canonical path no longer resolves to a regular file.)

# u64-form save bypasses the CLI's path-canonicalize step (which would fail
# with WEAVER-101 and exit 1 BEFORE dispatch under path-form). The dispatcher's
# R4 stat call then fires the path-missing arm.
weaver save "$ENTITY"
echo "exit code: $?"   # expect 0 (CLI dispatches; service refuses silently on the wire)
```

**Verify**:

- The TUI Buffers row does NOT flip to `clean`. Dirty persists.
- `weaver-buffers`'s stderr emits a `WEAVER-SAVE-006` warn record:
  ```
  WARN weaver_buffers: WEAVER-SAVE-006 path missing on save
    code="WEAVER-SAVE-006" entity=<entity-ref-u64> event_id=<uuidv8>
    path=/tmp/weaver-slice-005/file.txt
  ```
  No `expected_inode` / `actual_inode` fields — `WEAVER-SAVE-006` is structurally distinct from `-005` (path missing vs. inode mismatch; spec FR-016/FR-017 + §102.4).
- `ls /tmp/weaver-slice-005/file.txt` reports `No such file or directory` — the save did NOT recreate the file.
- The buffer's in-memory state is unchanged; the operator can re-create the file externally + restart `weaver-buffers` to recover.

**Why u64-form**: `weaver save <PATH>`'s resolver canonicalises the path via `std::fs::canonicalize` BEFORE dispatch (`core/src/cli/save.rs::resolve_entity`). After `rm`, the path doesn't resolve; the CLI exits 1 with `WEAVER-101` and never reaches the dispatcher. The u64-form skips canonicalisation and dispatches the `BufferSave` directly; the dispatcher's R4 step then sees the missing path and fires `-006`. The same applies to a plain `mv`-away.

**E2e coverage**: `tests/e2e/buffer_save_inode_refusal.rs::external_rename_away_..._fires_save_006` and `..._delete_..._fires_save_006` (SC-503).

## Scenario 4 — Atomic-rename invariant under I/O failure (SC-504)

**Setup**: this scenario uses an in-process test harness rather than a six-terminal walk; it is included here for completeness of the spec's verification surface. Operator-runnable verification is via `cargo test`.

**Action** (test-binary-driven; see `tests/e2e/buffer_save_atomic_invariant.rs`):

The test:
1. Sets up a `BufferState` opened on a tempfile with seed content.
2. Calls `BufferState::save_to_disk_with_hooks` (cross-crate via the `weaver_buffers::test_support` seam) with a hook closure that returns `Err(io::Error::other("ENOSPC (injected)"))` at a chosen `WriteStep`.
3. Asserts:
   - At every PRE-rename WriteStep (`OpenTempfile` / `WriteContents` / `FsyncTempfile` / `RenameToTarget`): the on-disk file is byte-identical to its pre-save state, the original inode is preserved, and no orphan `.weaver-save.<uuid>` tempfile remains in the parent directory (atomic-rename invariant SC-504).
   - At `FsyncParentDir` (POST-rename): `rename(2)` already swapped the directory entry, so disk reflects the new content + new inode. The dispatcher classifies this as `RenameIo` for surface uniformity, but the SC-504 atomicity invariant does not extend across rename completion (durability is the only concern).
   - The save outcome is `SaveOutcome::TempfileIo { error }` for steps 1–3 and `SaveOutcome::RenameIo { error }` for steps 4–5 (BufferState-level enum; the dispatcher-level `BufferSaveOutcome::TempfileIo` / `RenameIo` mapping happens at the wire/event layer in `dispatch_buffer_save`, which the dispatcher's unit tests cover separately).

**Verify** (test passes):

- `cargo test --test buffer_save_atomic_invariant` exits `0`. Three test functions: `production_save_writes_new_content_to_disk`, `rename_step_failure_preserves_original_and_cleans_tempfile`, `failure_at_every_writestep_outcome_and_atomicity`.

**E2e coverage**: `tests/e2e/buffer_save_atomic_invariant.rs` (SC-504).

## Scenario 5 — Multi-producer UUIDv8 prefix-uniqueness (US3, SC-505)

**Setup**: in-process test harness for stress-volume control; manual reproduction is impractical for routine quickstart use.

**Action** (test-binary-driven; see `tests/e2e/multi_producer_uuidv8.rs`):

The test:
1. Spawns a `weaver run` core (single binary process; the harness opens four bus connections to it).
2. Subscribes a single observer via `EventSubscribePattern::PayloadTypes(["buffer-edit","buffer-save","buffer-open"])` BEFORE any producer emits.
3. Spawns three concurrent producer tasks, each with its own `ActorIdentity` and prefix derivation:
   - **Producer A** (Service `producer-a`, `instance_id_A` = fresh UUIDv4): emits 1000 `BufferEdit` events on entity 1; prefix = `hash_to_58(&instance_id_A)`.
   - **Producer B** (Service `producer-b`, `instance_id_B` = fresh UUIDv4): emits 1000 `BufferSave` events on entity 2; prefix = `hash_to_58(&instance_id_B)`.
   - **Producer C** (`ActorIdentity::User`, per-process UUIDv4_C): emits 1000 `BufferOpen` events with unique paths; prefix = `hash_to_58(&UUIDv4_C)`.
   - Each producer uses a monotonic `0..1000` counter as `time_or_counter` (avoids sub-nanosecond clock-granularity collisions on fast hardware).
4. After all producers finish, drains 3000 events from the observer; for each event records `(EventId, prefix, provenance.source)`.
5. Asserts:
   - All 3000 EventIds are unique (no two events share an `EventId` — within-producer uniqueness from the counter; cross-producer uniqueness from the prefix-namespace partition).
   - Each producer's prefix maps to exactly 1000 events (no prefix-namespace leakage).
   - For every event, `extract_prefix(event.id)` matches the producer associated with `event.provenance.source` (within-slice trust assumption: well-behaved producers mint under their own prefix only).

**Verify** (test passes):

- `cargo test --test multi_producer_uuidv8` exits `0` in ~3s. The test's single function `three_producers_emit_3000_events_with_unique_uuidv8_ids` runs the full stress.

**E2e coverage**: `tests/e2e/multi_producer_uuidv8.rs` (SC-505).

## Scenario 6 — Codec strict-parsing rejection on malformed UUID (US3, SC-506)

**Setup**: also test-binary-driven; the scenario exercises a codec-layer rejection that operator tooling will not normally encounter (well-behaved clients always mint structurally-valid UUIDv8s).

**Action** (test-binary-driven; see `tests/e2e/eventid_uuid_strict_parsing.rs`):

The test:
1. Spawns a `weaver run` core.
2. Establishes a bus connection via `Client::connect` (handshake at protocol `0x05`).
3. Builds a structurally-valid `BusMessage::Event(Event)` with a normal UUIDv8 EventId, encodes via `ciborium::into_writer`, then walks the resulting CBOR `ciborium::Value` tree to patch the inner `Event.id` slot from a 16-byte byte string to an **8-byte byte string** (a length the `uuid` crate's serde `Deserialize` rejects — it requires exactly 16 bytes for a Uuid). Re-encodes the patched Value to bytes.
4. Sends the patched length-prefixed frame on the post-handshake socket via `client.stream.write_all`.
5. Asserts: `client.recv().await` returns `Err`. The listener's `read_message` returned `Err(CodecError::Decode(_))`; `run_message_loop` returned `Err`; cleanup ran; the socket closed. No `BusMessage::Error` is written to the client on codec failure (per the listener's actual semantics: `Incoming::Client(Err(e)) => return Err(e.into())` at `core/src/bus/listener.rs:226`); the connection close is the rejection signal.

**Verify**:

- `cargo test --test eventid_uuid_strict_parsing` exits `0`. The single test function asserts the closed-connection behavior.

**Note on scope** (per slice-005 session-1 narrowing of SC-506):

- "Wrong version nibble" rejection (a syntactically-valid 16-byte UUID with version != 8) is NOT exercised here. The `uuid` crate's `from_bytes` accepts any 16 bytes; version-bit enforcement is deferred to slice 006 alongside FR-029 (unauthenticated-edit/save-channel close-out).
- The pre-2026-04-29 SC-506 wording ("listener catches identity spoofing on producer-supplied `Event { id, .. }`") was narrowed under the §28(a) re-derivation because there is no `Event` / `EventOutbound` envelope split anymore — the codec accepts `Event` with `id`, and the only structural rejection at the codec layer is on malformed UUID bytes (length / type mismatches that ciborium-deserialise into `Uuid` cannot consume).

**E2e coverage**: `tests/e2e/eventid_uuid_strict_parsing.rs` (SC-506).

## Scenario 7 — Save against clean buffer (SC-507)

**Setup**: continuing from Scenario 1's clean post-save state. The buffer is at `version=1`, `dirty=false`. **NOTE**: if Scenarios 2 and 3 ran in between, the buffer is dirty and/or `weaver-buffers` was restarted; either re-establish Scenario 1's clean state with a fresh `weaver edit` + `weaver save` cycle, or run Scenario 7 immediately after Scenario 1 (the recommended walkthrough order).

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
    code="WEAVER-SAVE-007" entity=<entity-ref-u64> event_id=<uuidv8>
    path=/tmp/weaver-slice-005/file.txt version=1
  ```
- The TUI Buffers row remains in `clean` state. The annotation line refreshes — `event EventId(<n>)` now points at the latest BufferSave event (different from Scenario 1's id) — but the value `clean` is unchanged.
- `diff /tmp/before-mtime.txt /tmp/after-mtime.txt` exits `0` — the file's mtime was NOT touched. SC-507 verifies no disk I/O on clean save.
- `ENTITY=$(buffer_entity /tmp/weaver-slice-005/file.txt)`; `weaver inspect $ENTITY:buffer/dirty --why` walkback resolves to this latest BufferSave event (the most-recent re-assertion of `dirty=false`). The walkback IS to the no-op save, not to the original Scenario 1 save — this is the slice-004 precedent (FR-009 re-emits on every accepted operation).

**E2e coverage**: `tests/e2e/buffer_save_clean_noop.rs` (SC-507).

## Verification checklist

After completing scenarios 1–7, the operator has demonstrated:

- [x] **SC-501**: edit → save → `buffer/dirty = false` within interactive latency; on-disk content matches in-memory.
- [x] **SC-502**: external atomic-replace between open and save → `WEAVER-SAVE-005` + buffer state preserved + externally-written content preserved (no clobber).
- [x] **SC-503**: external delete or rename-away between open and save → `WEAVER-SAVE-006` + no file created + buffer state preserved (u64-form save required to bypass CLI canonicalize).
- [x] **SC-504**: I/O failure at any pre-rename `WriteStep` → original file byte-identical to pre-save state + tempfile cleaned up (test-binary verification; FsyncParentDir failure is post-rename and out of SC-504's scope).
- [x] **SC-505**: multi-producer stress → 3000 events, 100% unique EventIds, prefix-namespace partitioning verified across 3 producers × 1000 events (test-binary verification).
- [x] **SC-506**: codec strict-parsing rejection on malformed UUID payload → connection drops at the listener-side codec; no BusMessage::Error written (test-binary verification). Note: stronger spoofing-detection deferred to slice 006 + FR-029.
- [x] **SC-507**: clean-save no-op → `WEAVER-SAVE-007` + idempotent `buffer/dirty = false` re-emission + mtime preserved.

## Operator-pace notes

- **SC-501 latency**: the ≤500 ms budget is operator-perceived (TUI flip from `dirty` to `clean`). The actual save flow on commodity SSD typically completes in <20 ms; the budget has ~25× margin and is robust to filesystem variation. Use `scripts/measure_sc501.sh` to capture min/median/p95/max over N iterations (default N=20) for the T033 verification step.
- **SC-507 mtime check**: mtime granularity on most filesystems is 1 second (some are nanosecond-precise; a few are 2-second-resolution e.g. FAT). The `sleep 1` before the `before-mtime.txt` capture ensures a measurable mtime baseline; the post-save `after-mtime.txt` should match within filesystem granularity.
- **SC-505 / SC-506 are test-binary-only**: there is no realistic operator workflow that emits 3000 events in tight succession or that constructs a malformed wire frame manually. These scenarios verify implementation correctness, not operator UX.
- **Scenario ordering**: Scenarios 2 and 3 leave the buffer in `dirty` state on refusal (the dispatcher's R4 arm publishes nothing — refused saves are silent on the wire). Scenario 7 (mtime preservation) needs a clean buffer; if running 2/3 before 7, restart `weaver-buffers` + re-run Scenario 1's edit+save cycle to re-establish a clean state. Recommended order: 1 → 7 → 2 → 3 (or 1 → 7 alone for the SC-501/SC-507 happy path; 2/3 are independent refusal-arm exercises).
- **Walkthrough order matters for the mtime check (SC-507)**: Scenario 1 must run before Scenario 7 (Scenario 1's save is what writes the file with the mtime that Scenario 7 then preserves).

## Slice-005 carried-over hygiene

Per FR-025: `core/src/cli/edit.rs::handle_edit_json` grew a code comment at the post-parse step explaining why empty `[]` JSON does NOT short-circuit (asymmetric with positional zero-pair). This is implementation-internal and not directly operator-observable; the comment improves the path's reviewability for future contributors.

---

*Phase 1 — quickstart.md complete. CLAUDE.md SPECKIT-block update follows as the final Phase-1 deliverable.*
