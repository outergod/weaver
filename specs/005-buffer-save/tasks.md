---
description: "Task list for slice 005 — Buffer Save"
---

# Tasks: Buffer Save (Slice 005)

**Input**: Design documents from `/specs/005-buffer-save/`
**Prerequisites**: plan.md (✓), spec.md (✓), research.md (✓), data-model.md (✓), contracts/{bus-messages.md, cli-surfaces.md} (✓), quickstart.md (✓)

**Tests**: REQUIRED — included throughout (spec specifies SC-501..507 success criteria + Independent Tests on each user story; L2 P9 mandates scenario + property tests; slice-004 precedent maintains the test-cadence convention).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (`US1`, `US2`, `US3`)
- File paths are absolute from repo root
- Weaver-specific markers (`{surface:*}`, `{latency:*}`) are review notes per L2 Principles 7/8/14/15/18/20

## Weaver-specific marker scope (slice-005 inventory)

- **`{surface:bus}`** — wire-protocol changes; pair with `CHANGELOG.md` MAJOR entry per Amendment 1 (BREAKING) + L2 P8.
- **`{surface:cli}`** — `weaver` CLI grammar additions; pair with `CHANGELOG.md` MINOR additive entry.
- **`{latency:interactive}`** — single-save dispatch path (SC-501, ≤500 ms operator-perceived).
- **`{retraction}`** — NOT applied this slice. Save acceptance **re-asserts** the existing `buffer/dirty` fact key (overwrite, same `FactKey`, new `false` value); slice-003/004 retraction paths (SIGTERM retract, SIGKILL release_connection) are unchanged. P20 is satisfied by inheritance, not new paths.
- **`{schema-migration}`** — NOT applied this slice. No fact-family schema shape changes; `buffer/dirty` shape (`FactValue::Bool`) is unchanged from slice 003.
- **`{host-primitive}`** — NOT applied this slice. No Steel host primitive added.

## Path Conventions

Slice-005 extends existing crates in place; **no new workspace member**. Paths use the established workspace layout:

- `core/src/...` — wire types, listener, trace store, CLI subcommand handlers
- `buffers/src/...` — service consumer (publisher reader-loop arm + save method on `BufferState`)
- `git-watcher/src/...` — producer-side migration to `EventOutbound`
- `tests/e2e/...` — end-to-end scenarios

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Confirm baseline before introducing wire-incompatible changes.

- [ ] T001 Confirm clean baseline on branch `005-buffer-save` at `master @ f0ff3ae` (slice-004 PR #11 merge): run `scripts/ci.sh` end-to-end (clippy + fmt-check + build + test); confirm `cargo run --bin weaver -- --version` reports `bus_protocol: "0.4.0"`. Document the green baseline as the slice-005 starting point.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Land the new wire types, the §28(a) ID-stripped envelope shape, the bus-protocol bump, and the producer-side migration to `EventOutbound`. These changes are wire-breaking; ALL user stories depend on this phase being complete and CI-green.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [ ] T002 [P] {surface:bus} Bump `BUS_PROTOCOL_VERSION` from `0x04` to `0x05` and `BUS_PROTOCOL_VERSION_STR` from `"0.4.0"` to `"0.5.0"` in `core/src/types/message.rs`. Update the version-mismatch detail-string template in `core/src/bus/listener.rs` (the literal `"bus protocol 0x04 required; received 0x03"` → `"bus protocol 0x05 required; received 0x04"`). All four binaries (`weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`) inherit the bumped string in their `--version` output via the constant — no per-binary edits required.
- [ ] T003 {surface:bus} Add `EventOutbound { name: String, target: Option<EntityRef>, payload: EventPayload, provenance: Provenance }` struct in `core/src/types/event.rs` with `#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]` + `#[serde(deny_unknown_fields)]`. Field set is the slice-001 canonical `Event` shape minus `id`; `causal_parent` lives on `provenance.causal_parent` per slice-001 data-model.md (NOT duplicated at the envelope level). No `rename_all` — mirrors `Event`'s existing serde derive (snake_case on the wire). Add `Event::from_outbound(id: EventId, outbound: EventOutbound) -> Self` constructor copying all four fields. Add unit tests in module: JSON + CBOR round-trip on `EventOutbound`; `Event::from_outbound` field equivalence; `deny_unknown_fields` rejects an `id` field on inbound deserialisation (regression for SC-506). Re-export from `core/src/lib.rs`. Per `research.md §4`.
- [ ] T004 {surface:bus} Refactor `BusMessage` in `core/src/types/message.rs` from a non-generic enum to a generic `BusMessage<E>` parameterised over the event payload type. The `Event(...)` variant now uses `E` instead of `Event`. Add type aliases `pub type BusMessageInbound = BusMessage<EventOutbound>` and `pub type BusMessageOutbound = BusMessage<Event>`. Depends on T003. Update every `BusMessage` use-site across the workspace to specify direction (`BusMessageInbound` for read paths, `BusMessageOutbound` for write paths). Most sites resolve mechanically: producers' `write_message` becomes `BusMessageInbound`-typed; subscribers' `read_message` becomes `BusMessageOutbound`-typed. Per `research.md §1`.
- [ ] T005 {surface:bus} Update codec functions in `core/src/bus/codec.rs`: `read_message<R>(&mut R) -> Result<BusMessageInbound, CodecError>` and `write_message<W>(&mut W, msg: BusMessageOutbound)` for the listener side; the producer-side codec (used by CLI clients and other publishers) becomes `read_message` returning `BusMessageOutbound` and `write_message` accepting `BusMessageInbound`. Express this asymmetry as two parallel codec function pairs OR via direction-typed traits; pick whichever expresses the producer/listener asymmetry most clearly without duplication. Depends on T004. Unit tests cover round-trip in both directions for non-Event variants (which serialise identically across directions) and direction-asymmetric round-trip for `Event(...)` variants.
- [ ] T006 [P] {surface:bus} Add `EventPayload::BufferSave { entity: EntityRef, version: u64 }` variant in `core/src/types/event.rs`. Variant tag: `"buffer-save"` per Amendment 5. Update the `EventPayload::type_tag` impl (slice-004 T009A) to map the new variant to `"buffer-save"`. Add `buffer_save_wire_shape` test asserting adjacent-tag JSON output `{"type":"buffer-save","payload":{"entity":<u64>,"version":<u64>}}`. Independent of T003-T005 at the file level (different variant; same enum but additive).
- [ ] T007 {surface:bus} Add `next_event_id: AtomicU64` field on `TraceStore` in `core/src/trace/store.rs`. Initialise to `1` (skipping `EventId::ZERO`). Add `pub(crate) fn stamp_and_insert(&self, outbound: EventOutbound) -> Event` method: allocate fresh ID via `next_event_id.fetch_add(1, Ordering::Relaxed)`, construct stamped `Event` via `Event::from_outbound`, insert into `by_event` index keyed by stamped ID, return stamped event. Depends on T003 (imports `EventOutbound`). Unit tests: monotonic ID allocation across N successive calls; ZERO never returned; concurrent allocation across threads produces unique IDs (proptest with `Arc<TraceStore>` + multiple `tokio::spawn`). Per `research.md §5`.
- [ ] T008 {surface:bus} Update listener in `core/src/bus/listener.rs` to receive `BusMessageInbound::Event(EventOutbound)` from clients, call `trace_store.stamp_and_insert(outbound)` to allocate the stamped EventId, then broadcast `BusMessageOutbound::Event(stamped_event)` to subscribers and dispatch to in-process behaviors via the existing `dispatcher.process_event(stamped_event)` path. Depends on T003 + T004 + T005 + T007. The slice-004 `validate_event_envelope` ZERO-rejection on inbound `Event` is removed (structurally subsumed — inbound `EventOutbound` cannot carry an `id`); the `lookup_event_for_inspect` ZERO-short-circuit (slice-004 PR #11 commit `f0112d4`) is preserved per FR-024. Update `event_inspect_lookup_tests` and `event_envelope_validation_tests` to reflect the structural change (the envelope-validation regression for ZERO becomes a codec-level rejection of `id`-bearing inbound events; the lookup-side regression stays).
- [ ] T009 [P] {surface:bus} Migrate `core/src/cli/edit.rs` (`weaver edit`, `weaver edit-json`) to construct `EventOutbound` instead of producer-minted `Event` per FR-019. Replace `Event { id: EventId::new(now_ns()), payload, provenance, causal_parent }` → `EventOutbound { payload, provenance, causal_parent }`. Producers no longer mint `EventId` from wall-clock-ns; the listener stamps. Update existing `weaver edit` / `weaver edit-json` tests to construct `BusMessageInbound::Event(EventOutbound)` for inbound assertions.
- [ ] T010 [P] {surface:bus} Migrate `buffers/src/publisher.rs` poll-tick re-emissions and `bootstrap_tick` allocation to `EventOutbound` per FR-019. Replace `EventId::new(now_ns())` (poll path) and `EventId::new(now_ns().wrapping_add(idx))` (bootstrap path; slice-004 §28 fix `f0112d4`) with construction via `EventOutbound { payload, provenance, causal_parent }`. The `idx`-fold workaround from slice 004 §28 fix is no longer needed — listener-stamping guarantees uniqueness. Remove the `bootstrap_tick: Vec<EventId>` field on the publisher's state (slice-004 T008 introduction).
- [ ] T011 [P] {surface:bus} Migrate `git-watcher/src/publisher.rs` poll-tick re-emissions to `EventOutbound` per FR-019. Replace `Event { id: EventId::new(now_ns()), .. }` → `EventOutbound { payload, provenance, causal_parent }`. No semantic change to git-watching logic.
- [ ] T012 {surface:bus} Add CBOR + JSON round-trip property tests for `EventPayload::BufferSave` and `EventOutbound`-wrapped `BufferSave` over randomly-generated payloads in `core/src/types/event.rs::tests`. Depends on T003 + T006. Strategy: generators for `EntityRef` (random u64), `version` (random u64), `Provenance` (random `ActorIdentity`, fixed `causal_parent: None` for top-level — separate test for `Some(EventId)` round-trip). Asserts `parse(emit(x)) == x` for both wire formats.
- [ ] T013 {surface:bus} Update slice-003/004 `handshake_tests` regression test in `core/src/bus/listener.rs` so it asserts the new mismatch detail-string format `"bus protocol 0x05 required; received 0x04"`. Depends on T002. Confirm the in-process `UnixStream::pair()` regression continues to exercise the protocol-version rejection path. Add a new test asserting `0x03` and earlier are also rejected with the same error category (forward-incompatibility check).

**Checkpoint**: Foundation ready — `EventPayload::BufferSave` is on the wire; `EventOutbound` is the producer-side wire shape; the listener stamps EventIds; the protocol-version handshake rejects 0x04 clients; all four producer mint sites have migrated. Round-trip invariants hold. User-story work can now begin in parallel (if staffed).

---

## Phase 3: User Story 1 — Save an edited buffer to disk (Priority: P1) 🎯 MVP

**Goal**: An operator runs `weaver save <ENTITY>` against a dirty buffer; `weaver-buffers` validates ownership + version, performs an atomic disk write (tempfile + fsync + rename), re-emits `buffer/dirty = false`; the TUI flips to `clean` within the interactive latency class. Plus the buffer-not-opened CLI exit-1 path (US1 #4), the `--why` walk to the accepted save event (US1 #3 / SC-505 partial), and the clean-save no-op flow (FR-005 / SC-507).

**Independent Test**: An operator starts core + `weaver-buffers ./file.txt` + TUI, edits the buffer once via `weaver edit`, then runs `weaver save <entity>`. Within ≤500 ms the TUI shows `buffer/dirty = false`; `cat ./file.txt` shows the post-edit content; `buffer/version` is unchanged. `weaver inspect --why <entity>:buffer/dirty` walks back to the accepted `BufferSave` event. A second `weaver save` against the now-clean buffer emits `WEAVER-SAVE-007 "nothing to save"` at info level + idempotent `buffer/dirty = false` re-emission, no disk I/O.

### Implementation for User Story 1

- [ ] T014 [P] [US1] Extend `BufferState` in `buffers/src/model.rs` with a private immutable `inode: u64` field. Update `BufferState::open(path, ...)` to capture the inode at construction time via `std::fs::metadata(&canonical_path)?.ino()` (using `std::os::unix::fs::MetadataExt`). Per `research.md §2`. Unit tests: open a file with known inode, assert captured value matches `stat(path).st_ino`; opening a path that points to a directory returns `Err` (existing behavior); opening a symlink follows it and captures the underlying file's inode (POSIX `metadata` follows symlinks by default).
- [ ] T015 [P] [US1] Add `pub(crate) enum WriteStep { OpenTempfile, WriteContents, FsyncTempfile, RenameToTarget, FsyncParentDir }` and `pub(crate) fn atomic_write_with_hooks<F>(path: &Path, contents: &[u8], mut before: F) -> Result<(WriteStep, io::Error), io::Error> where F: FnMut(WriteStep) -> Result<(), io::Error>` in `buffers/src/model.rs` (or a new `buffers/src/atomic_write.rs` module if `model.rs` gets crowded). Per `research.md §3`. Implement the five-step sequence: open tempfile in same dir as target with name `.<basename>.weaver-save.<uuid-v4>`; write contents; `fsync(tempfile)`; `rename(2)` tempfile → target; `fsync(parent_dir)`. Each step calls `before(step)` first; if the hook returns `Err`, short-circuit and propagate (after best-effort tempfile cleanup if a tempfile was already opened). On any I/O error from the actual syscall, return the `(WriteStep, io::Error)` tuple as `Err`. Unit tests: happy-path success on a tempdir; injection of `Err(io::Error::new(ErrorKind::OutOfStorage, "ENOSPC"))` at each `WriteStep` produces matching error tuple; tempfile cleanup verified post-failure.
- [ ] T016 [US1] Add `pub fn save_to_disk(&self, path: &Path) -> SaveOutcome` method on `BufferState` in `buffers/src/model.rs`. Depends on T014 + T015. Define `pub enum SaveOutcome { Saved { path: PathBuf }, InodeMismatch { expected: u64, actual: u64 }, PathMissing, TempfileIo { error: io::Error }, RenameIo { error: io::Error } }` per `data-model.md §SaveOutcome`. Implement R4 + R5 of the dispatcher pipeline (`data-model.md §Validation rules`): stat path → check existence + regular-file + inode equality; on success, call `atomic_write_with_hooks(path, &self.content, |_| Ok(()))`; map `(WriteStep, io::Error)` to either `TempfileIo` (steps OpenTempfile / WriteContents / FsyncTempfile) or `RenameIo` (steps RenameToTarget / FsyncParentDir). Note: R1 ownership and R2 version-handshake and R3 dirty-branch live in `dispatch_buffer_save`, NOT in `save_to_disk`. Unit tests: happy-path save against a tempfile; inode-mismatch returns `InodeMismatch` with correct expected/actual; missing path returns `PathMissing`; tempfile-IO failure (via T015 hook injection) returns `TempfileIo`; rename-IO failure returns `RenameIo`. The in-memory `BufferState::content` is unchanged across all failure outcomes (purity invariant).
- [ ] T017 [US1] Add `pub(crate) enum BufferSaveOutcome { Saved { entity, path, version }, CleanSaveNoOp { entity, version }, StaleVersion { event_version, current_version }, NotOwned { entity }, InodeMismatch { entity, path, expected, actual }, PathMissing { entity, path }, TempfileIo { entity, path, error }, RenameIo { entity, path, error } }` and `pub(crate) fn dispatch_buffer_save(registry: &mut BufferRegistry, entity: EntityRef, version: u64, event_id: EventId) -> BufferSaveOutcome` in `buffers/src/publisher.rs`. Depends on T016. Mirror slice-004's `dispatch_buffer_edit` shape. Implement R1–R6 of the validation pipeline (`data-model.md §Validation rules`): R1 ownership check (`NotOwned` if not owned); R2 version handshake (`StaleVersion` if mismatched); R3 dirty-branch — consult `BufferState.last_dirty` (slice-003 field, already cached on the buffer state); on clean → `CleanSaveNoOp`; on dirty → R4–R5 via `BufferState::save_to_disk` and map `SaveOutcome` to `BufferSaveOutcome` by adding `entity` + `version` context. Unit tests cover all eight outcome variants (NotOwned via empty registry; StaleVersion via mismatched version; CleanSaveNoOp via clean-flagged buffer state; Saved by saving against a tempfile fixture; InodeMismatch / PathMissing / TempfileIo / RenameIo by setting up filesystem fixtures or hook injection per T015 pattern).
- [ ] T018 [US1] {latency:interactive} Wire reader-loop arm in `buffers/src/publisher.rs::reader_loop` to dispatch `BusMessage::Event(Event { payload: EventPayload::BufferSave { entity, version }, id: event_id, .. })` through `dispatch_buffer_save`. Depends on T017. Subscribe to `payload-type=buffer-save` events on bootstrap (alongside existing `payload-type=buffer-edit` from slice 004). On each outcome:
  - **`Saved { entity, path, version }`**: `tracing::info!` with structured fields (accepted-save event); publish `FactAssert(buffer/dirty, Bool(false))` with `causal_parent = Some(event_id)`. Update `BufferState.last_dirty = false`.
  - **`CleanSaveNoOp { entity, version }`**: `tracing::info!` `WEAVER-SAVE-007 "nothing to save: buffer was already clean"` with `entity`, `path`, `event_id`, `version`; publish `FactAssert(buffer/dirty, Bool(false))` (idempotent re-emission) with `causal_parent = Some(event_id)`. `BufferState.last_dirty` already `false`.
  - **`StaleVersion { event_version, current_version }`**: `tracing::debug!` `WEAVER-SAVE-002` with `entity`, `event_id`, `event_version`, `current_version`. No publish.
  - **`NotOwned { entity }`**: `tracing::debug!` with `reason="unowned-entity"`, `event_id`, `entity`. No publish.
  - **`InodeMismatch { entity, path, expected, actual }`**: `tracing::warn!` `WEAVER-SAVE-005` with `entity`, `path`, `event_id`, `expected_inode`, `actual_inode`. No publish.
  - **`PathMissing { entity, path }`**: `tracing::warn!` `WEAVER-SAVE-006` with `entity`, `path`, `event_id`. No publish.
  - **`TempfileIo { entity, path, error }`**: `tracing::error!` `WEAVER-SAVE-003` with `entity`, `path`, `event_id`, `errno`, `os_error`. No publish.
  - **`RenameIo { entity, path, error }`**: `tracing::error!` `WEAVER-SAVE-004` with `entity`, `path`, `event_id`, `errno`, `os_error`. No publish.
- [ ] T019 [P] [US1] {surface:cli} Add `weaver save <ENTITY> [--socket <PATH>]` subcommand grammar in `core/src/cli/args.rs` (clap derive) and register in `core/src/cli/mod.rs`. Positional `<ENTITY>` accepts both path form and `EntityRef`-stringified form (auto-detect: parse as `u64` first; on parse failure, treat as path).
- [ ] T020 [US1] {surface:cli} Add `weaver save` handler `pub async fn handle_save(args: SaveArgs) -> Result<()>` in `core/src/cli/save.rs` (NEW file). Depends on T019. Flow per `cli-surfaces.md §weaver save §Pre-dispatch flow`: resolve `<ENTITY>` → `entity: EntityRef` (canonicalise path if path-form); connect to bus; in-process inspect-lookup via existing `weaver_core::cli::inspect` library function for `<entity>:buffer/version`; on `FactNotFound` return WEAVER-SAVE-001 (exit 1); construct `EventOutbound { payload: EventPayload::BufferSave { entity, version: looked_up }, provenance: Provenance { source: ActorIdentity::User, .. }, causal_parent: None }`; dispatch via `BusMessageInbound::Event(outbound)`; exit 0. Unit tests stub the bus client to test buffer-not-opened path + happy-dispatch path.
- [ ] T021 [P] [US1] {surface:cli} Add `WEAVER-SAVE-001` (buffer not opened) miette diagnostic code in `core/src/cli/errors.rs`, with `#[diagnostic(code(...))]` derive macro and structured `help` string per `cli-surfaces.md §WEAVER-SAVE-NNN`. Service-side codes (-002 through -007) emit via `tracing` only; not registered as miette diagnostics. Independent of T020 (different file).
- [ ] T022 [US1] e2e test `tests/e2e/buffer_save_dirty.rs`: six-process scenario (core + git-watcher + buffer-service + subscriber + `weaver edit` + `weaver save` invocations as `Command::output()` short-lived processes). Depends on Phase 2 (wire types) + T018 (service consumer) + T020 (CLI handler) + T021 (diagnostic). Three test functions:
  - `dirty_save_flips_dirty_to_false_and_persists_disk`: bootstrap a buffer with `"initial content"`, run `weaver edit <PATH> 0:0-0:0 "PREFIX "`, observe `buffer/dirty = true`; run `weaver save <entity>`, observe `buffer/dirty = false` within a 5s structural-break deadline; assert `cat <PATH>` returns `"PREFIX initial content"`; assert `buffer/version` unchanged. Report observed wall-clock to stderr (informational SC-501 measurement; operator judges against ≤500 ms via T030).
  - `buffer_not_opened_returns_exit_1`: with NO `weaver-buffers` running, invoke `weaver save /tmp/nonexistent.txt`; assert exit code 1 + stderr contains "WEAVER-SAVE-001" + "buffer not opened".
  - `stale_version_save_silent_drops`: bootstrap a buffer; run `weaver edit` to bump to v=1; manually construct a stale BufferSave at v=0 via direct bus client; assert no `buffer/dirty` re-emission observed; assert service stderr emits WEAVER-SAVE-002 at debug level.
- [ ] T023 [US1] e2e test `tests/e2e/buffer_save_clean_noop.rs`: six-process scenario. Depends on T022 (reuses fixture pattern). Two test functions:
  - `clean_save_emits_save_007_and_idempotent_dirty_reemit`: bootstrap a buffer; run `weaver edit` then `weaver save` to reach `buffer/dirty = false`; capture the post-save `EventId` of the latest BufferSave event from the trace; run a second `weaver save` against the now-clean buffer; assert: service stderr emits `WEAVER-SAVE-007` at info level; a NEW `buffer/dirty = false` fact is asserted with `causal_parent` pointing at the second BufferSave (not the first); assert mtime of `<PATH>` is preserved across the second save (no disk I/O).
  - `clean_save_inspect_why_walks_to_latest_save`: after the above, run `weaver inspect --why <entity>:buffer/dirty --output=json`; assert the walkback resolves to the second BufferSave event (most-recent re-assertion); assert the event's provenance source type is `"user"`. Pins SC-505 partial (single-producer walkback resolution).

**Checkpoint**: User Story 1 fully functional and testable. The MVP is shippable here.

---

## Phase 4: User Story 2 — Refuse save when path/inode changed externally (Priority: P2)

**Goal**: External rename / atomic-replace / deletion of the underlying file between open and save MUST fire `WEAVER-SAVE-005` / `WEAVER-SAVE-006` and refuse the save (no clobber). The buffer's in-memory state and `buffer/dirty` flag are unchanged on refusal.

**Independent Test**: Open + edit a buffer; externally `mv ./file ./file.bak` (or `rm ./file`); run `weaver save`; assert no flip to `clean`, the moved/deleted target is unaffected, and the appropriate WEAVER-SAVE-NNN diagnostic appears on the service stderr.

### Implementation for User Story 2

- [ ] T024 [US2] e2e test `tests/e2e/buffer_save_inode_refusal.rs`: six-process scenario. Depends on T018 (dispatcher arm already implements R4 inode check) + T022 (fixture pattern). Three test functions:
  - `external_rename_between_open_and_save_fires_save_005`: bootstrap a buffer at `<PATH>`; run `weaver edit` to make it dirty; externally `mv <PATH> <PATH>.bak`; run `weaver save`; assert: CLI exits 0; service stderr emits `WEAVER-SAVE-005` at warn level with `expected_inode` matching open-time + `actual_inode=<missing>` or `<other-inode>`; assert `cat <PATH>.bak` returns the pre-edit content (preserved); assert nothing exists at `<PATH>`; assert `buffer/dirty` remains `true` (no flip to false). SC-502.
  - `external_delete_between_open_and_save_fires_save_006`: bootstrap a buffer; edit it; `rm <PATH>`; run `weaver save`; assert: service stderr emits `WEAVER-SAVE-006` at warn level; assert nothing exists at `<PATH>` (no recreation); assert `buffer/dirty` remains `true`. SC-503.
  - `external_atomic_replace_fires_save_005`: bootstrap a buffer; edit it; externally swap `<PATH>` for a different file (e.g., `mv <PATH>.new <PATH>` after creating `<PATH>.new` with different content) — produces a different inode at the same path; run `weaver save`; assert: service stderr emits `WEAVER-SAVE-005`; assert `cat <PATH>` returns the externally-written content (not the buffer's); assert `buffer/dirty` remains `true`. SC-502 acceptance scenario 3.

**Checkpoint**: User Story 2 covers external-mutation refusal scenarios. Adds no implementation tasks beyond Phase 3's dispatcher (the dispatcher's R4 step already implements the refusal); just adds three e2e tests.

---

## Phase 5: User Story 3 — `weaver inspect --why` resolves source events across concurrent producers (Priority: P3)

**Goal**: §28(a)'s ID-stamping invariant — under multi-producer stress, every `weaver inspect --why` walkback resolves to the correct source producer in 100% of cases. Plus the atomic-rename invariant under simulated I/O failure (SC-504), and the codec-rejection of `Event`-with-id from inbound channel (SC-506).

**Independent Test**: A stress harness with N=1000 events from K=3 producers produces 100% correct walkbacks. An I/O-failure injection at `RenameToTarget` step preserves the original disk file. A wire-shape rejection on `Event { id, .. }` inbound returns a structured codec decode error.

### Implementation for User Story 3

- [ ] T025 [US3] e2e test `tests/e2e/buffer_save_atomic_invariant.rs`: in-process test (no separate CLI processes) using `BufferState::save_to_disk` directly with `atomic_write_with_hooks` injection. Depends on T015 + T016. The test:
  1. Set up a `BufferState` opened on a tempfile with known content (`"original"`).
  2. Mutate `BufferState::content` in-memory to `"NEW content"` (simulate an applied edit).
  3. Call `BufferState::save_to_disk(path)` with the production hook (`|_| Ok(())`) — assert success, assert `cat path` returns `"NEW content"`, assert `buffer/dirty` would now compute as `false`.
  4. Reset: rewrite tempfile to `"original"`; mutate `BufferState::content` to `"NEW content 2"`.
  5. Call `BufferState::save_to_disk(path)` via a wrapper that injects `Err(io::Error::new(ErrorKind::OutOfStorage, "ENOSPC"))` at `WriteStep::RenameToTarget`. Assert: returns `SaveOutcome::RenameIo { error }`; `cat path` returns `"original"` byte-for-byte (atomic-rename invariant SC-504); `std::fs::read_dir(parent)` shows no `.weaver-save.<uuid>` orphan tempfiles (cleanup verified).
  6. Repeat for each `WriteStep` injection point (`OpenTempfile`, `WriteContents`, `FsyncTempfile`, `RenameToTarget`, `FsyncParentDir`); assert original-file preservation in every case.
- [ ] T026 [US3] e2e test `tests/e2e/multi_producer_stamping.rs`: in-process stress harness exercising §28(a) walkback resolution. Depends on Phase 2 (T002-T013 for the §28(a) infrastructure). The test:
  1. Set up an in-process core listener + trace store + 3 mock producers.
  2. Each producer emits 1000 events in tight succession via `BusMessageInbound::Event(EventOutbound)`:
     - Producer A: 1000 `BufferEdit` events on entity 1.
     - Producer B: 1000 `BufferSave` events on entity 1.
     - Producer C: 1000 `BufferOpen` events with random paths.
  3. After all producers finish, walk every accepted event's stamped EventId via `TraceStore::find_event(id)`; assert each lookup returns the unique stamped event whose provenance matches the originating producer's `ActorIdentity`.
  4. Assert: 100% walkback resolution — no two distinct accepted events share an EventId; every `find_event(id)` returns the unique event the listener stamped at that id.
  5. Assert: total stamped count == 3000 (no events lost, no events duplicated).
  Pins SC-505 (multi-producer walkback resolution validates §28(a) wall-clock-ns collision class is closed).
- [ ] T027 [US3] e2e test `tests/e2e/event_outbound_codec_validation.rs`: in-process wire-shape rejection test. Depends on T003 + T005 + T008. The test:
  1. Set up an in-process core listener.
  2. Establish a bus connection (handshake at protocol 0x05).
  3. Manually serialise a `BusMessage::Event` frame whose `Event` payload includes an `id: EventId` field (i.e., the slice-004-or-earlier outbound shape, NOT `EventOutbound`). Use raw `serde_json::to_writer` + `serde_cbor::to_writer` bypassing the typed codec (so the test can construct a malformed inbound shape that the codec wouldn't normally let it produce).
  4. Send the frame.
  5. Assert: the codec returns `Err(CodecError::FrameDecode(...))` because `EventOutbound` deserialisation rejected the unknown `id` field (per `deny_unknown_fields` in T003); the connection receives `BusMessage::Error { category: "decode", .. }`; the trace contains no entry for the rejected event (verified by `TraceStore::next_event_id` value before-and-after).
  Pins SC-506 (wire-shape rejection on producer-supplied `Event { id, .. }`).

**Checkpoint**: User Story 3 verifies §28(a) infrastructure correctness + atomic-rename invariant + codec rejection. All seven Success Criteria (SC-501..507) are now structurally covered by tests.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation lockstep, hygiene carryover, changelog, and release-readiness validation.

- [ ] T028 Update `docs/07-open-questions.md §28` to RESOLVED status per `research.md §11`. Status line changes to `RESOLVED`. Add a new paragraph at the top naming the resolution: "Resolved in slice 005: option (a), ID-stripped-envelope sub-variant. Producers serialise `EventOutbound` (no `id`); the listener stamps `Event { id, .. }` on accept. See `specs/005-buffer-save/spec.md` FR-019..FR-024 and `specs/005-buffer-save/research.md §1, §4`." Annotate the candidate resolutions: (a) `[ADOPTED — ID-stripped envelope]`, (b) `[NOT ADOPTED — would have left trace-store internal gap]`, (c) `[NOT ADOPTED — scope-explosive]`. Preserve the "Revisit triggers" section unchanged for archaeological context.
- [ ] T029 [P] Add code comment at `core/src/cli/edit.rs::handle_edit_json` post-parse step explaining why empty `[]` JSON does NOT short-circuit (asymmetric with positional zero-pair). Reference: slice-004 `spec.md §220` reserves wire-level empty `BufferEdit` as a future-tool handshake-probe affordance. Per FR-025 and slice-004 session-3 handoff carryover. Independent of T028 (different file).
- [ ] T030 [P] Update `CHANGELOG.md` with three logical changes:
  - **Promote slice-004's `[Unreleased]` section to versioned**: rename `## [Unreleased] — slice 004 "Buffer Edit"` to `## [0.4.0] — 2026-04-27 — slice 004 "Buffer Edit"` (the merge date of PR #11). Closes the slice-004 promotion gap (the [Unreleased] header has been stale since the slice merged on 2026-04-27).
  - **Add a new `## [Unreleased] — slice 005 "Buffer Save"` section ABOVE the freshly-versioned 0.4.0 section**, with subsections: `### Bus protocol 0.5.0 — BREAKING` listing `EventPayload::BufferSave` variant added, `EventOutbound` struct added, `BusMessage<E>` generic refactor, producer-side EventId minting removed (listener-stamped), `Hello.protocol_version` advances `0x04 → 0x05`; `### weaver CLI 0.4.0 — MINOR additive` listing `weaver save <ENTITY>` subcommand added, `WEAVER-SAVE-NNN` diagnostic taxonomy (codes -001 through -007), `weaver --version` JSON `bus_protocol` advances `0.4.0 → 0.5.0`; `### docs/07-open-questions.md §28 — RESOLVED` pointing at slice 005 spec/plan/research.
  - **Update the "Public surfaces tracked" summary block** at the top of CHANGELOG.md to reflect the slice-005 surface bumps: `Bus protocol v0.5.0 (was v0.4.0)`; `CLI surface weaver v0.4.0 (was v0.3.0)`; `weaver-buffers` / `weaver-git-watcher` / `weaver-tui` constant-driven `bus_protocol` field updates `0.4.0 → 0.5.0`.
  Independent of T028 / T029 (different file).
- [ ] T031 Tick all completed tasks in `specs/005-buffer-save/tasks.md` (this file) and confirm `scripts/ci.sh` runs green at HEAD (clippy + fmt-check + build + test all pass with no warnings). Per Amendment 6 — agent contributions MUST run `scripts/ci.sh` before proposing a commit.
- [ ] T032 Operator quickstart walkthrough — operator runs the seven scenarios from `specs/005-buffer-save/quickstart.md` (Scenarios 1, 2, 3, 7 are operator-runnable; Scenarios 4, 5, 6 are test-binary verification via `cargo test --test`). Operator confirms: TUI flips, on-disk content correctness, stderr diagnostic emission for each `WEAVER-SAVE-NNN` code, `weaver inspect --why` walkback resolution. **STOP-and-surface to operator** for any TUI-visual or mtime-preservation observation that disagrees with the quickstart spec — operator judges; do not fake pass/fail. Drift discovered during walkthrough lands as additional commits in this slice (per slice-003/004 PR-discipline rule "drift becomes follow-up commits in this slice, NOT new tasks").
- [ ] T033 Operator-judged SC-501 timing — measure `weaver edit + weaver save → buffer/dirty=false` median latency over N=20 successive saves. Capture: median, p95, max wall-clock; write to stderr in test-runner output. **STOP-and-surface to operator** with the captured timing; operator judges against the ≤500 ms median budget. Per slice-004 T028 precedent (operator-pace verification of latency-class commitments).
- [ ] T034 Final CI green confirmation + PR opening. Confirm: `scripts/ci.sh` exits 0 at HEAD with all clippy / fmt-check / build / test passing. `git log master..HEAD --oneline` reads as a coherent slice (Conventional Commits per Amendment 1; one logical change per commit; no `--no-verify` / `--no-gpg-sign`). Open PR via `gh pr create --base master --head 005-buffer-save` with title `feat(buffer-save): slice 005 — disk write-back + §28(a) core-assigned EventIds` and body summarising: the seven success criteria + their verification, the §28 resolution, and the slice-006 forward references (concurrent-mutation guard FR-026, save-as FR-027, agent emitter the unauthenticated-channel close-out). Per slice-004 T029 precedent.

---

## Dependencies and Story Completion Order

**Story dependency graph**:
- Setup (Phase 1, T001) → Foundational (Phase 2, T002-T013) → US1 (Phase 3, T014-T023) → US2 (Phase 4, T024) → US3 (Phase 5, T025-T027) → Polish (Phase 6, T028-T034).
- US1 is the MVP; US2 and US3 depend on US1's dispatcher infrastructure but add no new implementation (only e2e tests verifying inherited behaviors).
- Polish phase depends on all prior phases having landed.

**Within Phase 2**:
- T002 ← (no deps within phase)
- T003 ← (no deps within phase)
- T004 ← T003 (uses `EventOutbound`)
- T005 ← T004 (uses generic `BusMessage<E>`)
- T006 ← (no deps within phase; just adds an enum variant)
- T007 ← T003 (uses `EventOutbound`)
- T008 ← T003 + T004 + T005 + T007 (consumes all wire infrastructure)
- T009 ← T003 + T004 (uses `EventOutbound` + `BusMessageInbound`)
- T010 ← T003 + T004 (same)
- T011 ← T003 + T004 (same)
- T012 ← T003 + T006 (round-trip tests for new types)
- T013 ← T002 (handshake regression)

**Within Phase 3 (US1)**:
- T014 ← (no deps within phase; struct field addition)
- T015 ← (no deps within phase; new helper)
- T016 ← T014 + T015 (uses inode field + atomic_write_with_hooks)
- T017 ← T016 (uses save_to_disk)
- T018 ← T017 (wires reader-loop arm)
- T019 ← (no deps within phase; CLI grammar)
- T020 ← T019 (uses parsed args)
- T021 ← (no deps within phase; diagnostic registration)
- T022 ← T018 + T020 + T021 (full e2e setup)
- T023 ← T022 (reuses fixture pattern)

**Parallel-execution opportunities within Phase 2**:
- T002, T003, T006 can run in parallel (different files / different concerns).
- T009, T010, T011 can run in parallel after T003 + T004 (different producer crates).
- T013 can run in parallel with most other Phase 2 tasks (it tests an existing path).

**Parallel-execution opportunities within Phase 3 (US1)**:
- T014, T015 can run in parallel (different concerns within `model.rs` + new helper module).
- T019, T021 can run in parallel (different files in `core/src/cli/`).

## Implementation Strategy

**MVP scope**: Phase 1 + Phase 2 + Phase 3 (T001–T023). At MVP exit:
- Bus protocol bumped to 0x05 with `BufferSave` variant + `EventOutbound` shape.
- §28(a) ID-stamping infrastructure is in place; producer-side mint sites have migrated.
- `weaver save` CLI subcommand works for the dirty-buffer happy path, the buffer-not-opened error path, and the clean-save no-op path.
- Two e2e tests cover SC-501 and SC-507.

**Incremental delivery beyond MVP**:
- Phase 4 (US2) adds three e2e tests for SC-502 + SC-503 (one task, T024).
- Phase 5 (US3) adds three e2e tests for SC-504 + SC-505 + SC-506 (three tasks, T025–T027).
- Phase 6 (Polish) adds documentation, changelog, hygiene comment, operator walkthrough, timing verification, and PR-open.

**Test-cadence convention** (slice-003/004 inheritance): tests are written within the same phase as the implementation they verify. No "tests at the end" antipattern; each implementation task pairs with its scenario or property test in the same phase.

**Slice-completion gate**: CI green + operator-confirmed walkthrough + operator-judged SC-501 timing pass + all WEAVER-SAVE-NNN codes observed at expected levels + §28 doc updated + CHANGELOG entries landed.
