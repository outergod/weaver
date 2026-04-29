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
- `git-watcher/src/...` — producer-side migration to UUIDv8 mint with hashed `instance_id` prefix
- `tests/e2e/...` — end-to-end scenarios

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Confirm baseline before introducing wire-incompatible changes.

- [X] T001 Confirm clean baseline on branch `005-buffer-save` at `master @ f0ff3ae` (slice-004 PR #11 merge): run `scripts/ci.sh` end-to-end (clippy + fmt-check + build + test); confirm `cargo run --bin weaver -- --version` reports `bus_protocol: "0.4.0"`. Document the green baseline as the slice-005 starting point.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Land the new event variant, the protocol-version bump, and the §28(a) UUIDv8 EventId infrastructure (type cascade + producer-mint helper + producer-side migration). These changes are wire-breaking; ALL user stories depend on this phase being complete and CI-green.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

**Note on supersession**: This phase was originally framed (2026-04-27) around an ID-stripped-envelope direction (`EventOutbound` struct, generic `BusMessage<E>`, listener-side `stamp_and_insert`). The 2026-04-29 constitutional re-derivation (`research.md §12`) replaced that direction with producer-minted UUIDv8. Tasks T003, T004, T005, T007 of the original Phase 2 are SUPERSEDED — those landed in commits `7458c46`, `517aa75`, `4e3b9fc`, `069fa3e` and revert via the Step-2 revert commit. T002, T006, T013 stay valid. T008, T009, T010, T011, T012 are reframed under the UUIDv8 direction. New Phase 2A tasks (T-A1..T-A4) implement the UUIDv8 cascade + display layer.

- [X] T002 [P] {surface:bus} Bump `BUS_PROTOCOL_VERSION` from `0x04` to `0x05` and `BUS_PROTOCOL_VERSION_STR` from `"0.4.0"` to `"0.5.0"` in `core/src/types/message.rs`. Update the version-mismatch detail-string template in `core/src/bus/listener.rs` (the literal `"bus protocol 0x04 required; received 0x03"` → `"bus protocol 0x05 required; received 0x04"`). All four binaries (`weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`) inherit the bumped string in their `--version` output via the constant — no per-binary edits required. **STATUS**: landed in commit `2aef926` (slice-005 session 1); KEEP as-is.
- [X] ~~T003~~ **SUPERSEDED**. The `EventOutbound` struct + `Event::from_outbound` constructor were the 2026-04-27 ID-stripped-envelope direction. Replaced by producer-minted UUIDv8 (no envelope split). The original landing commit `7458c46` reverts as part of the Step-2 revert commit.
- [X] ~~T004~~ **SUPERSEDED**. The `BusMessage<E>` generic refactor + `BusMessageInbound`/`BusMessageOutbound` aliases were predicated on the inbound/outbound asymmetry from T003. Under UUIDv8 there is no asymmetry on the `Event` carrier. The original landing commit `517aa75` reverts as part of the Step-2 revert commit.
- [X] ~~T005~~ **SUPERSEDED**. The direction-typed codec siblings (`read_message_inbound`, `write_message_inbound`) were predicated on T004. Single `read_message`/`write_message` suffices under UUIDv8. The original landing commit `4e3b9fc` reverts as part of the Step-2 revert commit.
- [X] T006 [P] {surface:bus} Add `EventPayload::BufferSave { entity: EntityRef, version: u64 }` variant in `core/src/types/event.rs`. Variant tag: `"buffer-save"` per Amendment 5. Update the `EventPayload::type_tag` impl (slice-004 T009A) to map the new variant to `"buffer-save"`. Add `buffer_save_wire_shape` test asserting adjacent-tag JSON output `{"type":"buffer-save","payload":{"entity":<u64>,"version":<u64>}}`. **STATUS**: landed in commit `9f7f694` (slice-005 session 1); KEEP as-is — variant is independent of the §28 mechanism choice.
- [X] ~~T007~~ **SUPERSEDED**. The `TraceStore::next_event_id` `AtomicU64` counter + `stamp_and_insert` method were predicated on listener-side stamping. Under UUIDv8 the listener does not stamp; producers mint locally. The original landing commit `069fa3e` reverts as part of the Step-2 revert commit.
- [X] T008 {surface:bus} Update `validate_event_envelope` in `core/src/bus/listener.rs` so its slice-004 `EventId::ZERO`-rejection is retargeted at `EventId::nil()` (semantically unchanged; new wire shape). Preserve the slice-004 entity-match check on `BufferEdit` envelope/payload; add the analogous entity-match check on `BufferSave` (envelope `target == payload.entity`). UUIDv8-prefix-vs-provenance verification (catching a producer that mints under another producer's prefix) is **DEFERRED** to slice 006 alongside FR-029 — add a TODO comment with the FR-029 cross-reference. Depends on T-A1 (the `EventId(Uuid)` type cascade). Update `event_envelope_validation_tests` accordingly: ZERO-rejection becomes nil-rejection; entity-match regression extended to BufferSave.
- [X] T009 [P] {surface:bus} Migrate `core/src/cli/edit.rs` (`weaver edit`, `weaver edit-json`) to mint `EventId` via UUIDv8 with the per-process User-prefix (FR-019). Replace `EventId::new(now_ns())` → `EventId::mint_v8(get_user_prefix(), now_ns())`. The per-process User-prefix is a `OnceLock<u64>` initialised on first emit via `hash_to_58(&Uuid::new_v4())`. Depends on T-A1 + T-A2.
- [X] T010 [P] {surface:bus} Migrate `buffers/src/publisher.rs` poll-tick re-emissions and `bootstrap_tick` allocation to UUIDv8 with the Service-identity prefix (FR-019). Replace `EventId::new(now_ns())` (poll path) and `EventId::new(now_ns().wrapping_add(idx))` (bootstrap path; slice-004 §28 fix `f0112d4`) with `EventId::mint_v8(hash_to_58(&actor_identity.instance_id), now_ns())`. The `idx`-fold workaround is no longer needed — within-producer prefix-namespace + nanosecond low-bits guarantees uniqueness. The `bootstrap_tick: Vec<EventId>` field on the publisher's state is preserved (the producer needs to know its own minted ids to share them as `causal_parent` on the bootstrap facts — chain affordance is the load-bearing benefit of producer-minting). Depends on T-A1 + T-A2.
- [X] T011 [P] {surface:bus} Migrate `git-watcher/src/publisher.rs` poll-tick re-emissions to UUIDv8 with the Service-identity prefix (FR-019). Replace `Event { id: EventId::new(now_ns()), .. }` → `Event { id: EventId::mint_v8(hash_to_58(&actor_identity.instance_id), now_ns()), .. }`. No semantic change to git-watching logic. Depends on T-A1 + T-A2.
- [X] T012 {surface:bus} Add CBOR + JSON round-trip property tests for `EventPayload::BufferSave` and the new `EventId(Uuid)` shape over randomly-generated payloads in `core/src/types/event.rs::tests`. Depends on T006 + T-A1. Strategy: generators for `EntityRef` (random u64), `version` (random u64), `Provenance` (random `ActorIdentity`, fixed `causal_parent: None` for top-level — separate test for `Some(EventId)` round-trip with random UUIDv8 values). Asserts `parse(emit(x)) == x` for both wire formats.
- [X] T013 {surface:bus} Update slice-003/004 `handshake_tests` regression test in `core/src/bus/listener.rs` so it asserts the new mismatch detail-string format `"bus protocol 0x05 required; received 0x04"`. Depends on T002. Confirm the in-process `UnixStream::pair()` regression continues to exercise the protocol-version rejection path. Add a new test asserting `0x03` and earlier are also rejected with the same error category (forward-incompatibility check). **STATUS**: T013 was landed alongside T002 in commit `2aef926`; KEEP as-is.

**Checkpoint**: Phase 2 ready when (a) the Step-2 revert commit lands (removing T003/T004/T005/T007's wire infrastructure), (b) Phase 2A T-A1..T-A4 land (UUIDv8 cascade + mint helper + display layer), (c) T008/T009/T010/T011/T012 land under the UUIDv8 direction. All commits CI-green at every step. User-story work can begin in parallel after the checkpoint.

---

## Phase 2A: §28(a) UUIDv8 cascade + mint helper + display layer

**Purpose**: Cascade the `EventId(u64)` → `EventId(Uuid)` type change workspace-wide, add the UUIDv8 mint helper with hashed-producer-prefix logic, and add passive-cache display rendering for human-readable EventId display in TUI + `weaver inspect`. Sequenced AFTER the Step-2 revert commit (so the workspace returns to slice-004-plus-protocol-bump-plus-BufferSave-variant) and BEFORE Phase 2's T008/T009/T010/T011/T012 (which depend on the UUIDv8 type + helper).

- [X] T-A1 {surface:bus} `EventId(u64)` → `EventId(Uuid)` type cascade in `core/src/types/ids.rs`. Constructors: `EventId::nil()` (replaces `EventId::ZERO`), `EventId::for_testing(value: u128)` (deterministic test constructor wrapping `Uuid::from_u128`). All callers across the workspace cascade mechanically (~50+ sites): test fixtures `EventId::new(42)` → `EventId::for_testing(42)`; sentinel sites `EventId::ZERO` → `EventId::nil()` (including the slice-004 `lookup_event_for_inspect` ZERO-short-circuit at `core/src/bus/listener.rs` and `core/src/inspect/handler.rs`). Production callers temporarily wrap `now_ns()` via `Uuid::from_u128(now_ns() as u128)` so the workspace stays internally consistent at this commit; the producer-mint-site UUIDv8 migration happens in T-A2 + T009/T010/T011. Round-trip tests: existing CBOR/JSON round-trip pins continue to work (UUID has serde derives via `uuid` crate's `serde` feature; the wire shape changes from CBOR unsigned-int to CBOR byte-string, exercised by existing serde round-trip property tests). CI must stay green at this commit.
- [X] T-A2 {surface:bus} Add `EventId::mint_v8(producer_prefix_58: u64, time_or_counter: u64) -> Self` helper in `core/src/types/ids.rs` per `research.md §5`. Bit layout per RFC 9562 UUIDv8: high 58 bits of custom payload encode `producer_prefix_58`, version field set to `0x8`, variant bits set to `0b10`, low ~62 bits of custom payload encode `time_or_counter` (with 2 bits dropped to fit variant). Also add `EventId::extract_prefix(&self) -> u64` (recovers the 58-bit producer prefix from a UUIDv8 EventId — used by display layer T-A3/T-A4 for prefix → friendly_name lookup). Add a producer-prefix derivation helper in the same module (or in a new `core/src/types/producer_id.rs` if `ids.rs` gets crowded): `pub fn hash_to_58(uuid: &Uuid) -> u64` using `std::collections::hash_map::DefaultHasher` (SipHash). Unit tests: round-trip `mint_v8(prefix, time)` → `extract_prefix(...)` returns `prefix`; distinct `(prefix, time)` pairs map to distinct UUIDs; `hash_to_58` is deterministic on the same input. Per `research.md §5, §12`.
- [X] T-A3 [P] TUI passive-cache binding for prefix → friendly_name in `tui/src/render.rs` (or adjacent module). Maintain a per-process LRU cache keyed by 58-bit prefix → `(friendly_name: String, last_seen_ts: Instant)`. On every observed event/fact (fact path: `provenance.source` extraction; event path: `event.provenance.source`), update the cache entry for `extract_prefix(event.id)` → friendly_name derived from `ActorIdentity` (`ActorIdentity::Service { name, .. }` → `name`; `ActorIdentity::User` → `"user"`; `ActorIdentity::Agent` → `"agent"`; etc.). When rendering `event EventId(<n>)` annotations, look up the prefix in the local LRU cache; render `EventId(<friendly_name>/<short-hex-suffix>)` if known, full UUID hex otherwise (`EventId(0123456789abcdef...)`). Cache scope: per-client process; no persistence. Bootstrap miss is acceptable (renders raw UUID until a binding is observed). Independent of T-A4 (different file).
- [X] T-A4 [P] `weaver inspect` passive-cache binding for prefix → friendly_name in `core/src/cli/inspect.rs` (or new `core/src/cli/event_id_display.rs`). Same approach as T-A3 — LRU cache keyed by prefix → friendly_name; populated lazily during `weaver inspect --why` walkback rendering by extracting `provenance.source` from each fact/event encountered; for OLD events (pre-cache-warmup), client issues an existing `EventInspectRequest` by EventId → receives full Event with provenance → seeds the binding. Display format: `EventId(<friendly_name>/<short-hex-suffix>)` for known prefixes, full UUID via `--output=json`. Independent of T-A3 (different file).

**Checkpoint**: Phase 2A complete when T-A1 (type cascade) lands CI-green, T-A2 (mint helper + hash) lands CI-green, and T-A3/T-A4 (display passive-cache) land CI-green. After this point, T009/T010/T011 (producer-mint-site migrations) can land using the helper.

---

## Phase 3: User Story 1 — Save an edited buffer to disk (Priority: P1) 🎯 MVP

**Goal**: An operator runs `weaver save <ENTITY>` against a dirty buffer; `weaver-buffers` validates ownership + version, performs an atomic disk write (tempfile + fsync + rename), re-emits `buffer/dirty = false`; the TUI flips to `clean` within the interactive latency class. Plus the buffer-not-opened CLI exit-1 path (US1 #4), the `--why` walk to the accepted save event (US1 #3 / SC-505 partial), and the clean-save no-op flow (FR-005 / SC-507).

**Independent Test**: An operator starts core + `weaver-buffers ./file.txt` + TUI, edits the buffer once via `weaver edit`, then runs `weaver save <entity>`. Within ≤500 ms the TUI shows `buffer/dirty = false`; `cat ./file.txt` shows the post-edit content; `buffer/version` is unchanged. `weaver inspect --why <entity>:buffer/dirty` walks back to the accepted `BufferSave` event. A second `weaver save` against the now-clean buffer emits `WEAVER-SAVE-007 "nothing to save"` at info level + idempotent `buffer/dirty = false` re-emission, no disk I/O.

### Implementation for User Story 1

- [X] T014 [P] [US1] Extend `BufferState` in `buffers/src/model.rs` with a private immutable `inode: u64` field. Update `BufferState::open(path, ...)` to capture the inode at construction time via `std::fs::metadata(&canonical_path)?.ino()` (using `std::os::unix::fs::MetadataExt`). Per `research.md §2`. Unit tests: open a file with known inode, assert captured value matches `stat(path).st_ino`; opening a path that points to a directory returns `Err` (existing behavior); opening a symlink follows it and captures the underlying file's inode (POSIX `metadata` follows symlinks by default).
- [X] T015 [P] [US1] Add `pub(crate) enum WriteStep { OpenTempfile, WriteContents, FsyncTempfile, RenameToTarget, FsyncParentDir }` and `pub(crate) fn atomic_write_with_hooks<F>(path: &Path, contents: &[u8], mut before: F) -> Result<(WriteStep, io::Error), io::Error> where F: FnMut(WriteStep) -> Result<(), io::Error>` in `buffers/src/model.rs` (or a new `buffers/src/atomic_write.rs` module if `model.rs` gets crowded). Per `research.md §3`. Implement the five-step sequence: open tempfile in same dir as target with name `.<basename>.weaver-save.<uuid-v4>`; write contents; `fsync(tempfile)`; `rename(2)` tempfile → target; `fsync(parent_dir)`. Each step calls `before(step)` first; if the hook returns `Err`, short-circuit and propagate (after best-effort tempfile cleanup if a tempfile was already opened). On any I/O error from the actual syscall, return the `(WriteStep, io::Error)` tuple as `Err`. Unit tests: happy-path success on a tempdir; injection of `Err(io::Error::new(ErrorKind::OutOfStorage, "ENOSPC"))` at each `WriteStep` produces matching error tuple; tempfile cleanup verified post-failure.
- [X] T016 [US1] Add `pub fn save_to_disk(&self, path: &Path) -> SaveOutcome` method on `BufferState` in `buffers/src/model.rs`. Depends on T014 + T015. Define `pub enum SaveOutcome { Saved { path: PathBuf }, InodeMismatch { expected: u64, actual: u64 }, PathMissing, TempfileIo { error: io::Error }, RenameIo { error: io::Error } }` per `data-model.md §SaveOutcome`. Implement R4 + R5 of the dispatcher pipeline (`data-model.md §Validation rules`): stat path → check existence + regular-file + inode equality; on success, call `atomic_write_with_hooks(path, &self.content, |_| Ok(()))`; map `(WriteStep, io::Error)` to either `TempfileIo` (steps OpenTempfile / WriteContents / FsyncTempfile) or `RenameIo` (steps RenameToTarget / FsyncParentDir). Note: R1 ownership and R2 version-handshake and R3 dirty-branch live in `dispatch_buffer_save`, NOT in `save_to_disk`. Unit tests: happy-path save against a tempfile; inode-mismatch returns `InodeMismatch` with correct expected/actual; missing path returns `PathMissing`; tempfile-IO failure (via T015 hook injection) returns `TempfileIo`; rename-IO failure returns `RenameIo`. The in-memory `BufferState::content` is unchanged across all failure outcomes (purity invariant).
- [X] T017 [US1] Add `pub(crate) enum BufferSaveOutcome { Saved { entity, path, version }, CleanSaveNoOp { entity, version }, StaleVersion { event_version, current_version }, NotOwned { entity }, InodeMismatch { entity, path, expected, actual }, PathMissing { entity, path }, TempfileIo { entity, path, error }, RenameIo { entity, path, error } }` and `pub(crate) fn dispatch_buffer_save(registry: &mut BufferRegistry, entity: EntityRef, version: u64, event_id: EventId) -> BufferSaveOutcome` in `buffers/src/publisher.rs`. Depends on T016. Mirror slice-004's `dispatch_buffer_edit` shape. Implement R1–R6 of the validation pipeline (`data-model.md §Validation rules`): R1 ownership check (`NotOwned` if not owned); R2 version handshake (`StaleVersion` if mismatched); R3 dirty-branch — consult `BufferState.last_dirty` (slice-003 field, already cached on the buffer state); on clean → `CleanSaveNoOp`; on dirty → R4–R5 via `BufferState::save_to_disk` and map `SaveOutcome` to `BufferSaveOutcome` by adding `entity` + `version` context. Unit tests cover all eight outcome variants (NotOwned via empty registry; StaleVersion via mismatched version; CleanSaveNoOp via clean-flagged buffer state; Saved by saving against a tempfile fixture; InodeMismatch / PathMissing / TempfileIo / RenameIo by setting up filesystem fixtures or hook injection per T015 pattern).
- [X] T018 [US1] {latency:interactive} Wire reader-loop arm in `buffers/src/publisher.rs::reader_loop` to dispatch `BusMessage::Event(Event { payload: EventPayload::BufferSave { entity, version }, id: event_id, .. })` through `dispatch_buffer_save`. Depends on T017. Subscribe to `payload-type=buffer-save` events on bootstrap (alongside existing `payload-type=buffer-edit` from slice 004). On each outcome:
  - **`Saved { entity, path, version }`**: `tracing::info!` with structured fields (accepted-save event); publish `FactAssert(buffer/dirty, Bool(false))` with `causal_parent = Some(event_id)`. Update `BufferState.last_dirty = false`.
  - **`CleanSaveNoOp { entity, version }`**: `tracing::info!` `WEAVER-SAVE-007 "nothing to save: buffer was already clean"` with `entity`, `path`, `event_id`, `version`; publish `FactAssert(buffer/dirty, Bool(false))` (idempotent re-emission) with `causal_parent = Some(event_id)`. `BufferState.last_dirty` already `false`.
  - **`StaleVersion { event_version, current_version }`**: `tracing::debug!` `WEAVER-SAVE-002` with `entity`, `event_id`, `event_version`, `current_version`. No publish.
  - **`NotOwned { entity }`**: `tracing::debug!` with `reason="unowned-entity"`, `event_id`, `entity`. No publish.
  - **`InodeMismatch { entity, path, expected, actual }`**: `tracing::warn!` `WEAVER-SAVE-005` with `entity`, `path`, `event_id`, `expected_inode`, `actual_inode`. No publish.
  - **`PathMissing { entity, path }`**: `tracing::warn!` `WEAVER-SAVE-006` with `entity`, `path`, `event_id`. No publish.
  - **`TempfileIo { entity, path, error }`**: `tracing::error!` `WEAVER-SAVE-003` with `entity`, `path`, `event_id`, `errno`, `os_error`. No publish.
  - **`RenameIo { entity, path, error }`**: `tracing::error!` `WEAVER-SAVE-004` with `entity`, `path`, `event_id`, `errno`, `os_error`. No publish.
- [X] T019 [P] [US1] {surface:cli} Add `weaver save <ENTITY> [--socket <PATH>]` subcommand grammar in `core/src/cli/args.rs` (clap derive) and register in `core/src/cli/mod.rs`. Positional `<ENTITY>` accepts both path form and `EntityRef`-stringified form (auto-detect: parse as `u64` first; on parse failure, treat as path).
- [X] T020 [US1] {surface:cli} Add `weaver save` handler `pub async fn handle_save(args: SaveArgs) -> Result<()>` in `core/src/cli/save.rs` (NEW file). Depends on T019 + T-A1 + T-A2. Flow per `cli-surfaces.md §weaver save §Pre-dispatch flow`: resolve `<ENTITY>` → `entity: EntityRef` (canonicalise path if path-form); connect to bus; in-process inspect-lookup via existing `weaver_core::cli::inspect` library function for `<entity>:buffer/version`; on `FactNotFound` return WEAVER-SAVE-001 (exit 1); construct `Event { id: EventId::mint_v8(get_user_prefix(), now_ns()), name: "buffer/save", target: Some(entity), payload: EventPayload::BufferSave { entity, version: looked_up }, provenance: Provenance { source: ActorIdentity::User, timestamp_ns: now_ns(), causal_parent: None } }`; dispatch via `BusMessage::Event(event)`; exit 0. Unit tests stub the bus client to test buffer-not-opened path + happy-dispatch path.
- [X] T021 [P] [US1] {surface:cli} Add `WEAVER-SAVE-001` (buffer not opened) miette diagnostic code in `core/src/cli/errors.rs`, with `#[diagnostic(code(...))]` derive macro and structured `help` string per `cli-surfaces.md §WEAVER-SAVE-NNN`. Service-side codes (-002 through -007) emit via `tracing` only; not registered as miette diagnostics. Independent of T020 (different file).
- [X] T022 [US1] e2e test `tests/e2e/buffer_save_dirty.rs`: six-process scenario (core + git-watcher + buffer-service + subscriber + `weaver edit` + `weaver save` invocations as `Command::output()` short-lived processes). Depends on Phase 2 (wire types) + T018 (service consumer) + T020 (CLI handler) + T021 (diagnostic). Three test functions:
  - `dirty_save_flips_dirty_to_false_and_persists_disk`: bootstrap a buffer with `"initial content"`, run `weaver edit <PATH> 0:0-0:0 "PREFIX "`, observe `buffer/dirty = true`; run `weaver save <entity>`, observe `buffer/dirty = false` within a 5s structural-break deadline; assert `cat <PATH>` returns `"PREFIX initial content"`; assert `buffer/version` unchanged. Report observed wall-clock to stderr (informational SC-501 measurement; operator judges against ≤500 ms via T030).
  - `buffer_not_opened_returns_exit_1`: with NO `weaver-buffers` running, invoke `weaver save /tmp/nonexistent.txt`; assert exit code 1 + stderr contains "WEAVER-SAVE-001" + "buffer not opened".
  - `stale_version_save_silent_drops`: bootstrap a buffer; run `weaver edit` to bump to v=1; manually construct a stale BufferSave at v=0 via direct bus client; assert no `buffer/dirty` re-emission observed; assert service stderr emits WEAVER-SAVE-002 at debug level.
- [X] T023 [US1] e2e test `tests/e2e/buffer_save_clean_noop.rs`: six-process scenario. Depends on T022 (reuses fixture pattern). Two test functions:
  - `clean_save_emits_save_007_and_idempotent_dirty_reemit`: bootstrap a buffer; run `weaver edit` then `weaver save` to reach `buffer/dirty = false`; capture the post-save `EventId` of the latest BufferSave event from the trace; run a second `weaver save` against the now-clean buffer; assert: service stderr emits `WEAVER-SAVE-007` at info level; a NEW `buffer/dirty = false` fact is asserted with `causal_parent` pointing at the second BufferSave (not the first); assert mtime of `<PATH>` is preserved across the second save (no disk I/O).
  - `clean_save_inspect_why_walks_to_latest_save`: after the above, run `weaver inspect --why <entity>:buffer/dirty --output=json`; assert the walkback resolves to the second BufferSave event (most-recent re-assertion); assert the event's provenance source type is `"user"`. Pins SC-505 partial (single-producer walkback resolution).

**Checkpoint**: User Story 1 fully functional and testable. The MVP is shippable here.

---

## Phase 4: User Story 2 — Refuse save when path/inode changed externally (Priority: P2)

**Goal**: External rename / atomic-replace / deletion of the underlying file between open and save MUST fire `WEAVER-SAVE-005` / `WEAVER-SAVE-006` and refuse the save (no clobber). The buffer's in-memory state and `buffer/dirty` flag are unchanged on refusal.

**Independent Test**: Open + edit a buffer; externally `mv ./file ./file.bak` (or `rm ./file`); run `weaver save`; assert no flip to `clean`, the moved/deleted target is unaffected, and the appropriate WEAVER-SAVE-NNN diagnostic appears on the service stderr.

### Implementation for User Story 2

- [X] T024 [US2] e2e test `tests/e2e/buffer_save_inode_refusal.rs`: six-process scenario. Depends on T018 (dispatcher arm already implements R4 inode check) + T022 (fixture pattern). Three test functions:
  - `external_rename_between_open_and_save_fires_save_005`: bootstrap a buffer at `<PATH>`; run `weaver edit` to make it dirty; externally `mv <PATH> <PATH>.bak`; run `weaver save`; assert: CLI exits 0; service stderr emits `WEAVER-SAVE-005` at warn level with `expected_inode` matching open-time + `actual_inode=<missing>` or `<other-inode>`; assert `cat <PATH>.bak` returns the pre-edit content (preserved); assert nothing exists at `<PATH>`; assert `buffer/dirty` remains `true` (no flip to false). SC-502.
  - `external_delete_between_open_and_save_fires_save_006`: bootstrap a buffer; edit it; `rm <PATH>`; run `weaver save`; assert: service stderr emits `WEAVER-SAVE-006` at warn level; assert nothing exists at `<PATH>` (no recreation); assert `buffer/dirty` remains `true`. SC-503.
  - `external_atomic_replace_fires_save_005`: bootstrap a buffer; edit it; externally swap `<PATH>` for a different file (e.g., `mv <PATH>.new <PATH>` after creating `<PATH>.new` with different content) — produces a different inode at the same path; run `weaver save`; assert: service stderr emits `WEAVER-SAVE-005`; assert `cat <PATH>` returns the externally-written content (not the buffer's); assert `buffer/dirty` remains `true`. SC-502 acceptance scenario 3.

**Checkpoint**: User Story 2 covers external-mutation refusal scenarios. Adds no implementation tasks beyond Phase 3's dispatcher (the dispatcher's R4 step already implements the refusal); just adds three e2e tests.

---

## Phase 5: User Story 3 — `weaver inspect --why` resolves source events across concurrent producers (Priority: P3)

**Goal**: §28(a)'s ID-stamping invariant — under multi-producer stress, every `weaver inspect --why` walkback resolves to the correct source producer in 100% of cases. Plus the atomic-rename invariant under simulated I/O failure (SC-504), and the codec-rejection of `Event`-with-id from inbound channel (SC-506).

**Independent Test**: A stress harness with N=1000 events from K=3 producers produces 100% correct walkbacks. An I/O-failure injection at `RenameToTarget` step preserves the original disk file. A wire-shape rejection on `Event { id, .. }` inbound returns a structured codec decode error.

### Implementation for User Story 3

- [X] T025 [US3] e2e test `tests/e2e/buffer_save_atomic_invariant.rs`: in-process test (no separate CLI processes) using `BufferState::save_to_disk` directly with `atomic_write_with_hooks` injection. Depends on T015 + T016. The test:
  1. Set up a `BufferState` opened on a tempfile with known content (`"original"`).
  2. Mutate `BufferState::content` in-memory to `"NEW content"` (simulate an applied edit).
  3. Call `BufferState::save_to_disk(path)` with the production hook (`|_| Ok(())`) — assert success, assert `cat path` returns `"NEW content"`, assert `buffer/dirty` would now compute as `false`.
  4. Reset: rewrite tempfile to `"original"`; mutate `BufferState::content` to `"NEW content 2"`.
  5. Call `BufferState::save_to_disk(path)` via a wrapper that injects `Err(io::Error::new(ErrorKind::OutOfStorage, "ENOSPC"))` at `WriteStep::RenameToTarget`. Assert: returns `SaveOutcome::RenameIo { error }`; `cat path` returns `"original"` byte-for-byte (atomic-rename invariant SC-504); `std::fs::read_dir(parent)` shows no `.weaver-save.<uuid>` orphan tempfiles (cleanup verified).
  6. Repeat for each `WriteStep` injection point (`OpenTempfile`, `WriteContents`, `FsyncTempfile`, `RenameToTarget`, `FsyncParentDir`); assert original-file preservation in every case.
- [X] T026 [US3] e2e test `tests/e2e/multi_producer_uuidv8.rs`: in-process stress harness exercising §28(a) UUIDv8 prefix-uniqueness. Depends on Phase 2 + Phase 2A (T-A1, T-A2, T009, T010, T011). The test:
  1. Set up an in-process core listener + trace store + 3 mock producers, each with its own `ActorIdentity` (Service A with instance_id_A, Service B with instance_id_B, User C with per-process UUIDv4_C).
  2. Each producer emits 1000 events in tight succession via `BusMessage::Event(Event { id: EventId::mint_v8(<own_prefix>, now_ns()), .. })`:
     - Producer A: 1000 `BufferEdit` events on entity 1.
     - Producer B: 1000 `BufferSave` events on entity 1.
     - Producer C: 1000 `BufferOpen` events with random paths.
  3. After all producers finish, walk every accepted event's EventId via `TraceStore::find_event(id)`; assert each lookup returns the unique event whose provenance matches the originating producer's `ActorIdentity`.
  4. Assert: 100% walkback resolution — no two distinct accepted events share an EventId; every `find_event(id)` returns the unique event indexed at that id.
  5. Assert: total event count == 3000 (no events lost, no events duplicated).
  6. Assert: every event's `EventId::extract_prefix(...)` matches its producer's expected prefix (i.e., no producer's events leaked into another's prefix namespace — confirms the within-slice trust assumption holds in the test scenario).
  Pins SC-505 (multi-producer UUIDv8 prefix-uniqueness validates §28(a)'s cross-producer wall-clock-ns collision class is closed).
- [X] T027 [US3] e2e test `tests/e2e/eventid_uuid_strict_parsing.rs`: in-process codec strict-parsing rejection test. Depends on T-A1 + T002. The test:
  1. Set up an in-process core listener.
  2. Establish a bus connection (handshake at protocol 0x05).
  3. Manually construct a `BusMessage::Event(Event)` frame whose `id` is a malformed UUID payload — e.g., bytes that parse as a syntactically-valid UUID but with the wrong version nibble (not `0x8`), or 16 bytes that fail UUID parsing entirely. Use raw `serde_json::to_writer` + `ciborium::ser::into_writer` bypassing the typed codec.
  4. Send the frame.
  5. Assert: the codec returns `Err(CodecError::FrameDecode(...))` because the `uuid` crate's strict-parsing rejected the malformed UUID; the connection receives `BusMessage::Error { category: "decode", .. }`; the trace contains no entry for the rejected event.
  Pins SC-506 (codec strict-parsing rejection on malformed UUID payload).
  **Note**: this is a weaker version of the original 2026-04-27 SC-506 ("wire-shape rejection on producer-supplied `Event { id, .. }`"), narrowed under the 2026-04-29 re-derivation because there is no `Event` / `EventOutbound` envelope split anymore — the codec accepts `Event` with `id`, and the only structural rejection at the codec layer is on malformed UUID bytes. The stronger spirit of "listener rejects ill-formed inbound events" is split: malformed-UUID rejection ships in slice 005 (this task); UUIDv8-prefix-vs-provenance verification (catching identity spoofing) is DEFERRED to slice 006 alongside FR-029.

**Checkpoint**: User Story 3 verifies §28(a) infrastructure correctness + atomic-rename invariant + codec rejection. All seven Success Criteria (SC-501..507) are now structurally covered by tests.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Documentation lockstep, hygiene carryover, changelog, and release-readiness validation.

- [X] T028 Update `docs/07-open-questions.md §28` to RESOLVED status per `research.md §11`. **STATUS**: lands as part of the spec-amendment commit (Step 1 of the slice-005-session-1 rework plan), NOT as a separate Polish-phase commit. Status line changes to `RESOLVED`. New paragraph at the top names the resolution: "Resolved in slice 005 (2026-04-29 re-derivation): producer-minted UUIDv8 EventIds with hashed-producer-instance-id prefix. Service producers hash `ActorIdentity::Service::instance_id` to 58 bits; non-Service producers generate a per-process UUIDv4 and hash similarly. The listener does not stamp; producer's local id is final. Listener-side prefix-vs-provenance verification (catching identity spoofing) is DEFERRED to slice 006 alongside FR-029. The 2026-04-27 listener-stamping / ID-stripped-envelope direction is recorded as superseded; constitutional re-derivation rationale at `specs/005-buffer-save/research.md §12`. See also `specs/005-buffer-save/spec.md` FR-019..FR-024 + `specs/005-buffer-save/research.md §5, §12`." Annotate the candidate resolutions: (a) `[ADOPTED — UUIDv8 with hashed producer-instance-id prefix; spoofing-detection deferred to FR-029 close-out in slice 006]`, (b) `[NOT ADOPTED — TraceSequence proposal does not address producer-side mint hazard]`, (c) `[NOT ADOPTED — tuple form is wire-bloat + Hash/Eq complexity]`. Preserve the "Revisit triggers" section unchanged for archaeological context.
- [X] T029 [P] Add code comment at `core/src/cli/edit.rs::handle_edit_json` post-parse step explaining why empty `[]` JSON does NOT short-circuit (asymmetric with positional zero-pair). Reference: slice-004 `spec.md §220` reserves wire-level empty `BufferEdit` as a future-tool handshake-probe affordance. Per FR-025 and slice-004 session-3 handoff carryover. Independent of T028 (different file).
- [X] T030 [P] Update `CHANGELOG.md` with three logical changes:
  - **Promote slice-004's `[Unreleased]` section to versioned**: rename `## [Unreleased] — slice 004 "Buffer Edit"` to `## [0.4.0] — 2026-04-27 — slice 004 "Buffer Edit"` (the merge date of PR #11). Closes the slice-004 promotion gap (the [Unreleased] header has been stale since the slice merged on 2026-04-27).
  - **Add a new `## [Unreleased] — slice 005 "Buffer Save"` section ABOVE the freshly-versioned 0.4.0 section**, with subsections: `### Bus protocol 0.5.0 — BREAKING` listing `EventPayload::BufferSave` variant added, `EventId` wire-shape change `u64`→`Uuid` (UUIDv8 producer-minted with hashed-producer-instance-id prefix), `Hello.protocol_version` advances `0x04 → 0x05`; `### weaver CLI 0.4.0 — MINOR additive` listing `weaver save <ENTITY>` subcommand added, `WEAVER-SAVE-NNN` diagnostic taxonomy (codes -001 through -007), `weaver --version` JSON `bus_protocol` advances `0.4.0 → 0.5.0`; `### docs/07-open-questions.md §28 — RESOLVED` pointing at slice 005 spec/plan/research (note: the §28 doc update lands in the spec-amendment commit, not in CHANGELOG-time).
  - **Update the "Public surfaces tracked" summary block** at the top of CHANGELOG.md to reflect the slice-005 surface bumps: `Bus protocol v0.5.0 (was v0.4.0)`; `CLI surface weaver v0.4.0 (was v0.3.0)`; `weaver-buffers` / `weaver-git-watcher` / `weaver-tui` constant-driven `bus_protocol` field updates `0.4.0 → 0.5.0`.
  Independent of T028 / T029 (different file).
- [X] T031 Tick all completed tasks in `specs/005-buffer-save/tasks.md` (this file) and confirm `scripts/ci.sh` runs green at HEAD (clippy + fmt-check + build + test all pass with no warnings). Per Amendment 6 — agent contributions MUST run `scripts/ci.sh` before proposing a commit.
- [ ] T032 Operator quickstart walkthrough — operator runs the seven scenarios from `specs/005-buffer-save/quickstart.md` (Scenarios 1, 2, 3, 7 are operator-runnable; Scenarios 4, 5, 6 are test-binary verification via `cargo test --test`). Operator confirms: TUI flips, on-disk content correctness, stderr diagnostic emission for each `WEAVER-SAVE-NNN` code, `weaver inspect --why` walkback resolution. **STOP-and-surface to operator** for any TUI-visual or mtime-preservation observation that disagrees with the quickstart spec — operator judges; do not fake pass/fail. Drift discovered during walkthrough lands as additional commits in this slice (per slice-003/004 PR-discipline rule "drift becomes follow-up commits in this slice, NOT new tasks").
- [ ] T033 Operator-judged SC-501 timing — measure `weaver edit + weaver save → buffer/dirty=false` median latency over N=20 successive saves. Capture: median, p95, max wall-clock; write to stderr in test-runner output. **STOP-and-surface to operator** with the captured timing; operator judges against the ≤500 ms median budget. Per slice-004 T028 precedent (operator-pace verification of latency-class commitments).
- [ ] T034 Final CI green confirmation + PR opening. Confirm: `scripts/ci.sh` exits 0 at HEAD with all clippy / fmt-check / build / test passing. `git log master..HEAD --oneline` reads as a coherent slice (Conventional Commits per Amendment 1; one logical change per commit; no `--no-verify` / `--no-gpg-sign`). Open PR via `gh pr create --base master --head 005-buffer-save` with title `feat(buffer-save): slice 005 — disk write-back + §28(a) UUIDv8 EventIds` and body summarising: the seven success criteria + their verification, the §28 resolution (UUIDv8 with hashed producer-instance-id prefix; constitutional re-derivation rationale), and the slice-006 forward references (concurrent-mutation guard FR-026, save-as FR-027, agent emitter the unauthenticated-channel + UUIDv8-prefix-verification close-out). Per slice-004 T029 precedent.

---

## Dependencies and Story Completion Order

**Story dependency graph**:
- Setup (Phase 1, T001) → Foundational (Phase 2, partially landed; sequence: spec amendment → revert commit → Phase 2A T-A1..T-A4 → Phase 2 T008/T009/T010/T011/T012) → US1 (Phase 3, T014-T023) → US2 (Phase 4, T024) → US3 (Phase 5, T025-T027) → Polish (Phase 6, T028-T034).
- US1 is the MVP; US2 and US3 depend on US1's dispatcher infrastructure but add no new implementation (only e2e tests verifying inherited behaviors).
- Polish phase depends on all prior phases having landed.

**Within Phase 2 (post-2026-04-29 re-derivation)**:
- T002 ← landed `2aef926`; KEEP.
- ~~T003~~ SUPERSEDED → reverts via Step-2 revert commit.
- ~~T004~~ SUPERSEDED → reverts via Step-2 revert commit.
- ~~T005~~ SUPERSEDED → reverts via Step-2 revert commit.
- T006 ← landed `9f7f694`; KEEP.
- ~~T007~~ SUPERSEDED → reverts via Step-2 revert commit.
- T008 ← T-A1 (uses `EventId(Uuid)`)
- T009 ← T-A1 + T-A2 (uses `EventId::mint_v8` + `hash_to_58`)
- T010 ← T-A1 + T-A2 (same)
- T011 ← T-A1 + T-A2 (same)
- T012 ← T006 + T-A1 (round-trip tests for new types)
- T013 ← T002 (handshake regression); landed alongside `2aef926`; KEEP.

**Within Phase 2A**:
- T-A1 ← (no deps within phase; type cascade)
- T-A2 ← T-A1 (UUIDv8 mint helper uses `EventId(Uuid)`)
- T-A3 ← T-A2 (TUI passive-cache uses `extract_prefix`)
- T-A4 ← T-A2 (inspect passive-cache uses `extract_prefix`)

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

**Parallel-execution opportunities within Phase 2 + 2A**:
- T009, T010, T011 can run in parallel after T-A1 + T-A2 (different producer crates).
- T-A3, T-A4 can run in parallel after T-A2 (different files; passive-cache rendering in TUI vs `weaver inspect`).

**Parallel-execution opportunities within Phase 3 (US1)**:
- T014, T015 can run in parallel (different concerns within `model.rs` + new helper module).
- T019, T021 can run in parallel (different files in `core/src/cli/`).

## Implementation Strategy

**MVP scope**: Phase 1 + Phase 2 + Phase 2A + Phase 3 (T001–T023, T-A1..T-A4). At MVP exit:
- Bus protocol bumped to 0x05 with `BufferSave` variant + `EventId(Uuid)` shape.
- §28(a) UUIDv8 producer-mint infrastructure is in place; producer-side mint sites have migrated; passive-cache display layer renders human-readable EventIds.
- `weaver save` CLI subcommand works for the dirty-buffer happy path, the buffer-not-opened error path, and the clean-save no-op path.
- Two e2e tests cover SC-501 and SC-507.

**Incremental delivery beyond MVP**:
- Phase 4 (US2) adds three e2e tests for SC-502 + SC-503 (one task, T024).
- Phase 5 (US3) adds three e2e tests for SC-504 + SC-505 + SC-506 (three tasks, T025–T027).
- Phase 6 (Polish) adds documentation, changelog, hygiene comment, operator walkthrough, timing verification, and PR-open.

**Test-cadence convention** (slice-003/004 inheritance): tests are written within the same phase as the implementation they verify. No "tests at the end" antipattern; each implementation task pairs with its scenario or property test in the same phase.

**Slice-completion gate**: CI green + operator-confirmed walkthrough + operator-judged SC-501 timing pass + all WEAVER-SAVE-NNN codes observed at expected levels + §28 doc updated + CHANGELOG entries landed.
