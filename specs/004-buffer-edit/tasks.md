---

description: "Task list for slice 004 — Buffer Edit"
---

# Tasks: Buffer Edit (Slice 004)

**Input**: Design documents from `/specs/004-buffer-edit/`
**Prerequisites**: plan.md (✓), spec.md (✓), research.md (✓), data-model.md (✓), contracts/{bus-messages.md, cli-surfaces.md} (✓), quickstart.md (✓)

**Tests**: REQUIRED — included throughout (spec specifies SC-401..406 success criteria + Independent Tests on each user story; L2 P9 mandates scenario + property tests; slice-003 precedent maintains the test-cadence convention).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (`US1`, `US2`, `US3`)
- File paths are absolute from repo root
- Weaver-specific markers (`{surface:*}`, `{latency:*}`) are review notes per L2 Principles 7/8/14/15/18/20

## Weaver-specific marker scope (slice-004 inventory)

- **`{surface:bus}`** — wire-protocol changes; pair with `CHANGELOG.md` MAJOR entry per Amendment 1 (BREAKING) + L2 P8.
- **`{surface:cli}`** — `weaver` CLI grammar additions; pair with `CHANGELOG.md` MINOR additive entry.
- **`{latency:interactive}`** — single-edit dispatch path (SC-401, ≤500 ms operator-perceived).
- **`{retraction}`** — NOT applied this slice. Edit acceptance **re-asserts** existing fact keys (overwrite, same `FactKey`); slice-003 retraction paths (SIGTERM retract, SIGKILL release_connection) are unchanged. P20 is satisfied by inheritance, not new paths.
- **`{schema-migration}`** — NOT applied this slice. No fact-family schema shape changes; `buffer/version` shape is unchanged from PR #10's bootstrap forward-compat.
- **`{host-primitive}`** — NOT applied this slice. No Steel host primitive added.

## Path Conventions

Slice-004 extends existing crates in place; **no new workspace member**. Paths use the established workspace layout:

- `core/src/...` — wire types, listener, CLI subcommand handlers
- `buffers/src/...` — service consumer (publisher reader-loop arm + apply path)
- `tests/e2e/...` — end-to-end scenarios

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Confirm baseline before introducing wire-incompatible changes.

- [x] T001 Confirm clean baseline on branch `004-buffer-edit` at `master @ 655b77a`: run `scripts/ci.sh` end-to-end (clippy + fmt-check + build + test); confirm `cargo run --bin weaver -- --version` reports `bus_protocol: "0.3.0"`. Document the green baseline as the slice-004 starting point.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Land the new wire types + bus-protocol bump. These changes are wire-breaking; ALL user stories depend on this phase being complete and CI-green.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [x] T002 [P] {surface:bus} Create `core/src/types/edit.rs` with `Position { line: u32, character: u32 }`, `Range { start: Position, end: Position }`, `TextEdit { range: Range, new_text: String }` struct types. Apply `#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]` plus `#[serde(rename_all = "kebab-case")]` on `TextEdit` (Rust `new_text` ↔ JSON `new-text`). Re-export from `core/src/types/mod.rs` and `core/src/lib.rs`. Add unit tests in the module: JSON round-trip on each type asserting kebab-case wire form; lexicographic ordering of `Position`; UTF-8 `is_char_boundary` checks for `Position.character`.
- [x] T003 [P] {surface:bus} Bump `BUS_PROTOCOL_VERSION` from `0x03` to `0x04` and `BUS_PROTOCOL_VERSION_STR` from `"0.3.0"` to `"0.4.0"` in `core/src/types/message.rs`. Update the version-mismatch detail-string template in `core/src/bus/listener.rs` (the literal `"bus protocol 0x03 required; received 0x02"` → `"bus protocol 0x04 required; received 0x03"`). All four binaries (`weaver`, `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`) inherit the bumped string in their `--version` output via the constant — no per-binary edits required.
- [x] T004 {surface:bus} Add `EventPayload::BufferEdit { entity: EntityRef, version: u64, edits: Vec<TextEdit> }` variant in `core/src/types/event.rs`. Depends on T002 (imports `TextEdit`). Variant tag: `"buffer-edit"` per Amendment 5. Update the existing `buffer_open_wire_shape` test pattern: add `buffer_edit_wire_shape` test asserting adjacent-tag JSON output and the kebab-case `new-text` field on serialised `TextEdit`s.
- [x] T005 {surface:bus} Update the existing `handshake_tests` regression test in `core/src/bus/listener.rs` (introduced in slice 003) so it asserts the new mismatch detail-string format `"bus protocol 0x04 required; received 0x03"`. Depends on T003. Confirm the in-process `UnixStream::pair()` regression continues to exercise the protocol-version rejection path.
- [x] T006 {surface:bus} Add CBOR + JSON round-trip property tests for `EventPayload::BufferEdit` over randomly-generated `Vec<TextEdit>` (proptest) in `core/src/types/event.rs::tests`. Depends on T004. Strategy: generators for `Position` (bounded line/character u32), `Range` (start ≤ end lexicographically), `TextEdit` (random ASCII `new_text` for proptest determinism), `Vec<TextEdit>` of length 0..32. Asserts `parse(emit(x)) == x` for both wire formats.

**Checkpoint**: Foundation ready — `EventPayload::BufferEdit` is on the wire; the protocol-version handshake rejects 0x03 clients; round-trip invariants hold. User-story work can now begin in parallel (if staffed).

---

## Phase 3: User Story 1 — Apply a single edit to an opened buffer (Priority: P1) 🎯 MVP

**Goal**: An operator runs `weaver edit <PATH> <RANGE> <TEXT>` to apply a single edit to an opened buffer; `weaver-buffers` validates + applies + bumps `buffer/version` + re-emits facts; the TUI reflects the new state within the interactive latency class. Plus the buffer-not-opened CLI exit-1 path (US1 #4) and the `--why` walk to the accepted event (US1 #5, SC-405).

**Independent Test**: An operator starts core + `weaver-buffers ./file.txt` + TUI, waits for bootstrap, runs `weaver edit ./file.txt 0:0-0:0 "hello "`, and observes `buffer/version=1` + new `buffer/byte-size` + `buffer/dirty=true` in the TUI. The on-disk file is unchanged. `weaver inspect --why <entity>:buffer/version` walks back to the accepted `BufferEdit` event (`ActorIdentity::User` provenance).

### Implementation for User Story 1

- [x] T007 [P] [US1] Add `BufferState::apply_edits(&mut self, edits: &[TextEdit]) -> Result<(), ApplyError>` in `buffers/src/model.rs`. Depends on T002 (imports `TextEdit`). Define `enum ApplyError { OutOfBounds { edit_index: usize, detail: String }, MidCodepointBoundary { edit_index: usize, side: BoundarySide, line: u32, character: u32 }, IntraBatchOverlap { first_index: usize, second_index: usize }, NothingEdit { edit_index: usize }, SwappedEndpoints { edit_index: usize }, InvalidUtf8 { edit_index: usize } }` and `enum BoundarySide { Start, End }` per `data-model.md §Validation rules`. Implement two-phase pipeline: (a) per-edit checks fail-fast (R1..R6); (b) sort-by-start + linear overlap-scan; (c) descending-offset apply via line-index table; (d) recompute `memory_digest` once at end. Unit tests in module cover all 6 ApplyError variants + happy paths (single edit, batched edit, pure-insert, pure-delete) + empty batch (`edits: []`) returns `Ok(())` with `memory_digest` byte-identical pre/post (FR-008) + memory_digest invariant preservation on accept + memory_digest unchanged on any rejection.
- [x] T008 [US1] Refactor `weaver-buffers` publisher state in `buffers/src/publisher.rs` from `Vec<BufferState>` + `BufferRegistry { owned: HashSet<EntityRef> }` to `BufferRegistry { buffers: HashMap<EntityRef, BufferState>, versions: HashMap<EntityRef, u64> }` per `research.md §7`. Bootstrap-tick deterministic event-IDs (currently derived from CLI argv index in slice-003 `open_and_bootstrap_all`) move to a separate `Vec<EventId>` captured at parse time. Slice-003 unit tests `dispatch_buffer_open_is_noop_for_already_owned_entity` and `dispatch_buffer_open_passes_observer_errors_through` continue to pass against the refactored registry. Initialise `versions[entity] = 0` at bootstrap (matches PR #10's `buffer/version=0` initial fact).
- [x] T009 [US1] Add `pub(crate) enum BufferEditOutcome { Applied(EntityRef, u64 /* new_version */, BufferState /* post-apply snapshot */), NotOwned, StaleVersion { current: u64, emitted: u64 }, FutureVersion { current: u64, emitted: u64 }, ValidationFailure(ApplyError) }` and `pub(crate) fn dispatch_buffer_edit(registry: &mut BufferRegistry, entity: EntityRef, version: u64, edits: &[TextEdit]) -> BufferEditOutcome` in `buffers/src/publisher.rs`. Depends on T007 + T008. Mirror slice-003's `dispatch_buffer_open` shape: pure-ish (no bus writes, no tracing beyond debug lines), unit-testable without a mock writer. Unit tests cover all five outcome variants (NotOwned via empty registry; StaleVersion / FutureVersion via mismatched version; ValidationFailure by passing a malformed batch; Applied by passing a valid batch). On `Applied`, the outcome carries the new version (current+1) and the post-apply state snapshot for the caller to use.

> **Mid-flight scope addition (T009A → T009C)** — discovered while planning T010: slices 001–003 only deliver `FactEvent` (Asserted/Retracted) to subscribers; `BusMessage::Event` reaches the trace + in-process behaviors via `dispatcher.process_event` but is **not** broadcast to external bus clients. The slice-004 plan implicitly assumed event delivery to subscribers; that infrastructure does not exist yet. Tasks T009A/T009B/T009C close the gap inside this slice rather than fragment to a slice 003.5 (operator decision 2026-04-25). See `research.md §13`. The bus protocol stays at 0x04 — these additions land within the same slice-004 wire envelope.

- [x] T009A [US1] {surface:bus} Add `EventSubscribePattern { PayloadType(String) }` enum in `core/src/types/message.rs`, `BusMessage::SubscribeEvents(EventSubscribePattern)` variant on the existing `BusMessage` enum (adjacent-tag `"subscribe-events"`; payload uses kebab-case; `payload-type` field carries the pattern's string), and `pub fn type_tag(&self) -> &'static str` on `EventPayload` (`"buffer-open"` / `"buffer-edit"` mapped from the variant). Unit tests: JSON + CBOR round-trip on `EventSubscribePattern` and `BusMessage::SubscribeEvents` asserting kebab-case wire form; `EventPayload::type_tag` matches the serde `#[serde(rename_all = "kebab-case")]` discriminant exactly (regression: derive a discriminant and compare). Depends on Phase 2 (T002–T006) and T009 already done.
- [x] T009B [US1] {surface:bus} Add `core/src/bus/event_subscriptions.rs` module with `EventSubscriptions { subscribers: Mutex<Vec<EventSubscriber>> }` registry holding per-connection `mpsc::UnboundedSender<Event>`s keyed by id; `EventSubscriptionHandle { rx, id }` returned from `subscribe(EventSubscribePattern)`; `broadcast(&self, event: &Event)` retains-by-send-success (mirrors `FactStore`'s broadcast pattern). Wire it through `Dispatcher` (new `event_subscriptions: Arc<EventSubscriptions>` field; `process_event` calls `self.event_subscriptions.broadcast(&event)` after the trace append + behavior fire). Wire `BusMessage::SubscribeEvents` into `core/src/bus/listener.rs`: `handle_client_message` returns the `EventSubscriptionHandle` parallel to the existing `SubscriptionHandle`; `run_message_loop` adds a third `select!` arm draining the event-rx and forwarding `BusMessage::Event` to the client; ack with the existing `BusMessage::SubscribeAck { sequence: 0 }`. Unit + integration tests: a 2-client topology (publisher + subscriber) where the subscriber receives events whose payload-type matches and not events whose payload-type differs; subscriber disconnect drops the subscription on next broadcast. Depends on T009A.
- [x] T009C [US1] {surface:bus} Modify `buffers/src/publisher.rs::run` to send `BusMessage::SubscribeEvents(EventSubscribePattern::PayloadType("buffer-edit".into()))` immediately after handshake (between `Lifecycle(Ready)` receipt and the per-buffer bootstrap loop); wait for `SubscribeAck`. Extend `reader_loop` to forward incoming `BusMessage::Event(_)` frames to a new `event_tx: mpsc::Sender<Event>` channel (parallel to the existing `err_tx`). Extend the main loop's `select!` with a `maybe_event = event_rx.recv()` arm that for now invokes a no-op `handle_event` stub (T010 fills it with the actual `dispatch_buffer_edit` plumbing). Depends on T009A + T009B; the no-op stub keeps T010's scope narrowly on the dispatcher arm.
- [x] T010 [US1] {latency:interactive} Wire reader-loop arm in `buffers/src/publisher.rs::reader_loop` to dispatch `BusMessage::Event(Event { payload: EventPayload::BufferEdit { entity, version, edits }, id: event_id, .. })` through `dispatch_buffer_edit`. Depends on T009 + T009A + T009B + T009C. On each outcome:
  - **`Applied(entity, new_version, _)`**: `tracing::info!` with structured fields; publish `FactAssert(buffer/byte-size, U64(new_byte_size))`, `FactAssert(buffer/version, U64(new_version))`, `FactAssert(buffer/dirty, Bool(memory_digest != sha256(disk_content)))` in that order, all with `causal_parent = Some(event_id)`.
  - **`NotOwned`**: `tracing::debug!` with `reason="unowned-entity"`, `event_id`, `entity`. No publish.
  - **`StaleVersion { current, emitted }`** / **`FutureVersion { current, emitted }`**: `tracing::debug!` with `reason="stale-version"` or `"future-version"`, `event_id`, `entity`, `emitted_version`, `current_version`. No publish.
  - **`ValidationFailure(err)`**: `tracing::debug!` with `reason="validation-failure-<kind>"` (kind derived from `ApplyError` variant), `event_id`, `entity`, `edit_index` (from the variant). No publish.
- [x] T011 [P] [US1] {surface:cli} Add `weaver edit <PATH> [<RANGE> <TEXT>]* [--socket <PATH>]` subcommand grammar in `core/src/cli/args.rs` (clap derive) and register in `core/src/cli/mod.rs`. Depends on T002 (transitively, via the `<RANGE>` parser in T012). Variadic positional pairs accepted via clap's `num_args(0..)` + post-parse pair validation (must have even count; if odd, fail with WEAVER-EDIT-002).
- [x] T012 [P] [US1] {surface:cli} Add `parse_range(&str) -> Result<Range, RangeParseError>` in `core/src/cli/edit.rs` parsing `<start-line>:<start-char>-<end-line>:<end-char>` (decimal u32 components). Depends on T002. Unit tests cover happy path, invalid format (missing component, non-decimal, swapped colons/dash), and overflow (u32::MAX boundary).
- [x] T013 [US1] {surface:cli} Add `weaver edit` handler `pub async fn handle_edit(args: EditArgs) -> Result<()>` in `core/src/cli/edit.rs`. Depends on T011 + T012. Flow per `cli-surfaces.md §weaver edit §Pre-dispatch flow`: canonicalise path; derive entity via `weaver_buffers::model::buffer_entity_ref` (cross-crate dep already exists in tests; for the CLI it's a runtime dep); connect to bus; run in-process inspect-lookup via existing `weaver_core::cli::inspect` library function for `<entity>:buffer/version`; on `FactNotFound` return WEAVER-EDIT-001 (exit 1); construct `Event { payload: EventPayload::BufferEdit { entity, version: looked_up, edits } }` with `Provenance { source: ActorIdentity::User, .. }`; dispatch via `BusMessage::Event`; exit 0. Unit tests stub the bus client to test buffer-not-opened path + happy-dispatch path + zero-positional-pair invocation emits `warn`-level stderr "no edits provided; nothing dispatched" and exits 0 without dispatching an event (FR-014).
- [x] T014 [P] [US1] {surface:cli} Add `WEAVER-EDIT-001` (buffer not opened) and `WEAVER-EDIT-002` (invalid range grammar) miette diagnostic codes in `core/src/cli/errors.rs`, with `#[diagnostic(code(...))]` derive macros and structured `help` strings per `cli-surfaces.md §Diagnostic codes`. Independent of T013 (different file).
- [x] T015 [US1] e2e test `tests/e2e/buffer_edit_single.rs`: five-process scenario (core + git-watcher + buffer-service + subscriber + `weaver edit` invocation as a `Command::output()` short-lived process). Depends on Phase 2 (wire types) + T010 (service consumer) + T013 (CLI handler) + T014 (diagnostics). Two test functions:
  - `single_edit_lands_with_version_bump_and_dirty_flip`: bootstrap a buffer, run `weaver edit <PATH> 0:0-0:0 "PREFIX "`, observe `buffer/version=1` + `buffer/byte-size+7` + `buffer/dirty=true` within a 5s structural-break deadline; report observed wall-clock to stderr (informational SC-401 measurement; operator judges against ≤500 ms via T028).
  - `buffer_not_opened_returns_exit_1`: with NO `weaver-buffers` running, invoke `weaver edit /tmp/nonexistent.txt 0:0-0:0 "x"`; assert exit code 1 + stderr contains "WEAVER-EDIT-001" + "buffer not opened".
> **Mid-flight scope addition (T016A → T016B)** — discovered while planning T016: SC-405 + FR-016 commit slice 004 to "`weaver inspect --why <entity>:buffer/version` walks back to the BufferEdit event's emitter `ActorIdentity`", but `--why` doesn't exist yet. The server-side `TraceStore::find_event` lookup is already there (slice 001); only the bus and CLI plumbing was missing. Operator decision (2026-04-25): close the gap inside slice 004 rather than ship a slice that fails its own committed acceptance criterion. See `research.md §14`.

- [x] T016A [US1] {surface:bus} Add `BusMessage::EventInspectRequest { request_id: u64, event_id: EventId }` + `BusMessage::EventInspectResponse { request_id: u64, result: Result<Event, EventInspectionError> }` + `EventInspectionError { EventNotFound }` in `core/src/types/message.rs`. Wire the listener handler in `core/src/bus/listener.rs`: look up the event via `dispatcher.trace().find_event(event_id)` → `TraceStore::get(seq)` → match the `TracePayload::Event` variant; respond with `Ok(event.clone())` or `Err(EventInspectionError::EventNotFound)`. Unit + JSON-round-trip tests for the new message variants; integration test for the listener happy-path + not-found path. Depends on Phase 2 (T002–T006) + the slice-004 wire envelope. Stays at protocol 0x04.
- [x] T016B [US1] {surface:cli} Add `--why` flag on `weaver inspect` in `core/src/cli/args.rs`. Extend `core/src/cli/inspect.rs::run` with a chained second round-trip when `--why` is set: take `source_event` from the returned `InspectionDetail` and issue `BusMessage::EventInspectRequest`; render the walkback JSON shape (per `cli-surfaces.md §weaver inspect --why`) with `fact`, `fact_inspection`, and `event` blocks. Exit 2 on `EventNotFound` (mirrors fact-not-found's exit-2 convention for "expected miss"). Unit test for the CLI argument parsing.
- [x] T016 [US1] e2e test `tests/e2e/buffer_edit_inspect_why.rs`: bootstrap a buffer, run `weaver edit` once, then run `weaver inspect --why <entity>:buffer/version --output=json`; assert the JSON walks back to the `BufferEdit` event (`event.id == fact_inspection.source_event`) with `event.provenance.source.type == "user"` per Amendment 5 wire shape. Pins SC-405. Depends on T015 (reuses fixture pattern) + T016A + T016B.

**Checkpoint**: User Story 1 fully functional and testable. The MVP is shippable here.

---

## Phase 4: User Story 2 — Atomic batched edits (Priority: P2)

**Goal**: A multi-edit `weaver edit <PATH> <R1> <T1> <R2> <T2> <R3> <T3>` invocation produces exactly one `buffer/version` bump and one fact-re-emission burst. Validation failure on any edit drops the entire batch with no observable mutation.

**Independent Test**: With a buffer at `version=N`, run a 16-edit batch with all-valid edits; assert single version bump to `N+1` and one re-emission burst. Then with `version=N+1`, run a 3-edit batch where the middle edit is out-of-bounds; assert `version` stays at `N+1`, no fact re-emission, no in-memory mutation observable to the subscriber.

### Implementation for User Story 2

- [x] T017 [P] [US2] Property test in `buffers/src/model.rs::tests` (or split into `buffers/tests/apply_edits_property.rs` if the unit-test module gets crowded): `apply_edits_accepts_iff_batch_is_structurally_valid`. Generator: random `BufferState` (from random UTF-8 content, bounded length) + random `Vec<TextEdit>` (bounded length 0..32). Property: `apply_edits` returns `Ok(())` ↔ batch satisfies R1..R6 + no intra-batch overlap (per `data-model.md §Validation rules`). On `Err`, `state.memory_digest` is structurally unchanged. On `Ok`, `state.memory_digest == sha256(state.content())`.
- [x] T018 [US2] e2e test `tests/e2e/buffer_edit_atomic_batch.rs`: same five-process pattern as T015. Two test functions:
  - `sixteen_edit_happy_batch_lands_atomically`: 16-edit batch where all edits are bounds-valid + non-overlapping; assert exactly one `buffer/version` bump observed (subscriber records all `buffer/version` updates; assert exactly one new entry post-dispatch); assert one set of three re-emitted facts (`buffer/byte-size`, `buffer/version`, `buffer/dirty`) sharing one `causal_parent`.
  - `three_edit_batch_with_invalid_middle_rejects_whole_batch`: three-edit batch where edit index 1 has out-of-bounds range; assert `buffer/version` stays at `N`, no fact re-emission, in-memory content (verified via subsequent successful single-edit + memory inspection through TUI/inspect path) is byte-identical to pre-dispatch state.

**Checkpoint**: User Story 2 fully functional and testable. Both US1 and US2 work independently.

---

## Phase 5: User Story 3 — JSON-driven edit input (Priority: P3)

**Goal**: A CI pipeline / integration script / operator pipes a JSON `Vec<TextEdit>` into `weaver edit-json <PATH> --from -` (or `--from <FILE>`) and gets identical observable behaviour to the equivalent `weaver edit` positional invocation.

**Independent Test**: Pipe a one-edit JSON array via stdin to `weaver edit-json` against an opened buffer; assert observable behaviour matches the equivalent `weaver edit` positional form. Pipe malformed JSON; assert exit 1 with WEAVER-EDIT-003 and no event dispatched.

### Implementation for User Story 3

- [x] T019 [P] [US3] {surface:cli} Add `weaver edit-json <PATH> [--from <PATH>|-] [--socket <PATH>]` subcommand grammar in `core/src/cli/args.rs` (clap derive) and register in `core/src/cli/mod.rs`. Depends on T002. The `--from` flag is REQUIRED (no implicit-stdin default per `cli-surfaces.md`); missing `--from` fails parse with usage diagnostic.
- [x] T020 [US3] {surface:cli} Add `weaver edit-json` handler `pub async fn handle_edit_json(args: EditJsonArgs) -> Result<()>` in `core/src/cli/edit.rs`. Depends on T019 + T013 (shares the dispatch path). Flow per `cli-surfaces.md §weaver edit-json §Pre-dispatch flow`: read JSON from stdin (`-`) or named file; parse to `Vec<TextEdit>` via serde_json; on parse failure return WEAVER-EDIT-003 (exit 1); reuse T013's canonicalise + connect + inspect-lookup + envelope construction; pre-serialise the constructed envelope via `ciborium::into_writer` and check `buf.len() <= weaver_core::bus::codec::MAX_FRAME_SIZE` (64 KiB); on overflow return WEAVER-EDIT-004 (exit 1); dispatch + exit 0. Refactor T013's handler so the canonicalise + inspect-lookup + dispatch path is a private library function reused by both subcommands. Unit tests cover happy path, malformed JSON, oversized payload.
- [x] T021 [P] [US3] {surface:cli} Add `WEAVER-EDIT-003` (malformed edit-json input) and `WEAVER-EDIT-004` (serialised BufferEdit exceeds wire-frame) miette diagnostic codes in `core/src/cli/errors.rs` per `cli-surfaces.md §Diagnostic codes`. Independent of T020.
- [x] T022 [US3] property test `tests/e2e/buffer_edit_emitter_parity.rs`: pin SC-406. Generator: random `Vec<TextEdit>` (bounded length 1..16, bounded `new_text` length, bounded line/character u32). Property: for every generated batch B, the bytes written by `weaver edit <PATH> <pairs-of-B>` and `weaver edit-json <PATH> --from <json-of-B>` to the bus (captured via a test-harness `UnixListener` substituting for the core) are byte-identical (after stripping non-deterministic envelope fields like `Provenance.timestamp_ns` and `EventId`, which differ per invocation). The deterministic core of the `EventPayload::BufferEdit` payload (`entity`, `version`, `edits`) MUST match exactly. 256-case proptest harness with shrinking.

**Checkpoint**: All three user stories functional and independently testable.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Coverage of remaining SCs (sequential/structural SC-403, stale-drop SC-404), CHANGELOG, operator-judged SC-401 wall-clock, final CI gate, PR readiness.

- [x] T023 [P] e2e test `tests/e2e/buffer_edit_sequential.rs`: pin SC-403 (structural-only, no wall-clock budget per spec Q4). Bootstrap a buffer; run 100 sequential `weaver edit` invocations with single-byte `<TEXT>` payloads; capture `buffer/version` updates from a long-lived subscriber; assert `version=100` after the loop completes; assert NO gaps in the observed update sequence (every value 1..=100 appears exactly once); report total wall-clock to stderr informationally.
- [x] T024 [P] e2e test `tests/e2e/buffer_edit_stale_drop.rs`: pin SC-404. Synchronisation primitive (e.g., `tokio::sync::Barrier` or a process-coordinated stage marker via filesystem) ensures both `weaver edit` invocations complete their pre-dispatch lookup before either dispatches. Spawn two `weaver edit` processes; assert exactly one `buffer/version` bump observed (the winner); the loser's CLI exits 0 (fire-and-forget, no detection); assert the service's stderr (captured via `RUST_LOG=weaver_buffers=debug`) contains a line with `reason="stale-version"`, `current_version=1`, `emitted_version=0`.
- [ ] T025 {surface:bus} {surface:cli} Update `CHANGELOG.md`. Promote slice-003's `[Unreleased]` body (the `buffer/version` forward-compat entry from PR #10) to a `[0.3.0a]` or fold it into the new entry. Add a fresh `[Unreleased]` section with two subsections: (a) **Bus protocol 0.4.0** — `EventPayload::BufferEdit` variant + `TextEdit`/`Range`/`Position` struct types; `Hello.protocol_version` 0x03 → 0x04; mismatch detail-string format; BREAKING. (b) **Weaver CLI MINOR additive** — `weaver edit` and `weaver edit-json` subcommands; `bus_protocol` JSON field advances 0.3.0 → 0.4.0 in all four binaries. Reference slice-004 spec/plan/contracts.
- [ ] T026 docs(tasks): mark T001..T025 complete (Phases 1–6 pre-operator-validation). Pure-docs commit per slice-003 convention; mirrors `f98ea07` / `eab2338` / etc.
- [ ] T027 (OPERATOR-REQUIRED) Run `specs/004-buffer-edit/quickstart.md` walkthrough end-to-end. Operator executes Scenarios 1–6, validates each acceptance criterion by hand, and reports any drift between spec and observed behaviour. Drift becomes follow-up commits in this slice (NOT new tasks). The agent surfaces this task as STOP-AND-SURFACE; the agent MUST NOT report this complete without operator confirmation.
- [ ] T028 (OPERATOR-WALL-CLOCK) SC-401 single-edit timing. Run `tests/e2e/buffer_edit_single.rs::single_edit_lands_with_version_bump_and_dirty_flip` 5 times; capture observed wall-clock from stderr; report min / median / max to operator. Hardware-dependent; operator judges pass/fail against the ≤500 ms budget. The agent MUST NOT mark this passed without operator confirmation.
- [ ] T029 Final `scripts/ci.sh` green gate + PR readiness. Agent runs `scripts/ci.sh` on the full slice-004 branch (clippy + fmt-check + build + full test suite). On green, agent reports "ready for PR" and surfaces the slice-completion checklist: every commit Conventional-Commits-conformant; every task in Phases 1–6 marked complete; T027 + T028 operator-confirmed; CHANGELOG.md current; every public-surface change documented per L2 P7/P8.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies; can start immediately.
- **Foundational (Phase 2)**: Depends on Setup; **BLOCKS all user stories**. T002 + T003 are parallel; T004 depends on T002; T005 depends on T003; T006 depends on T004.
- **User Stories (Phase 3+)**: All depend on Foundational completion. User stories are **mostly independent**; cross-story dependencies are noted below.
- **Polish (Phase 6)**: T023, T024 parallel with each other after Phase 5; T025 (CHANGELOG) can land any time after Phase 2 but is convention-placed at slice end; T026 bookkeeping after T023..T025; T027 + T028 operator-gates; T029 final gate.

### User Story Dependencies

- **US1 (P1)**: Independent after Phase 2. Mostly self-contained; T013 (handler) depends on T011 + T012; T015 + T016 are e2e tests at story end. T009A/T009B/T009C close the mid-flight event-broadcast infrastructure gap (see `research.md §13`); they are blockers for T010 and any e2e test that exercises the publisher's edit-receive path.
- **US2 (P2)**: Depends on Phase 2 + US1 implementation tasks (T007 `apply_edits` is shared; T008 `BufferRegistry` shared; T010 reader-loop shared). The US2 phase adds the property test (T017) + the atomic-batch e2e (T018); these don't introduce new production code paths.
- **US3 (P3)**: Depends on Phase 2 + US1 implementation. T019 + T020 add the JSON subcommand grammar + handler; T020 refactors T013's handler to share a private library function. T022 property test depends on T019 + T020 + T013.

### Within Each User Story

- Tests assert against production code paths; **production code first, tests second** (the slice-003 convention is "test commits follow implementation commits"; not strict TDD red-green).
- Models before services: T007 (model) before T009/T010 (service).
- Services before CLI integration: T010 (service consumer) before T015/T016 (e2e exercising the round-trip).
- For tasks that assert facts: P20 retraction discipline is satisfied by inheritance from slice-003's existing retraction paths (no new fact key is introduced; edit-acceptance overwrites existing keys).
- For tasks tagged `{surface:bus}` and `{surface:cli}`: pair with `CHANGELOG.md` entries (T025).

### Parallel Opportunities

- **Phase 2**: T002 || T003 (different files, independent).
- **Phase 3**: T011 || T012 || T014 (different CLI files, independent of each other after T002 lands).
- **Phase 4**: T017 (property test) is `[P]` with itself only — single task in this phase.
- **Phase 5**: T019 || T021 (different files).
- **Phase 6**: T023 || T024 (different e2e test files, independent of each other after Phase 5 lands).

### Cross-story / cross-phase parallelism

After Phase 2 lands, US1 / US2 / US3 implementation work could proceed in parallel if staffed (different team members, different files). In a single-agent sequential implementation flow, however, the natural order is US1 (MVP) → US2 → US3 → Polish, matching the priority gradient.

---

## Parallel Example: Phase 2

```bash
# T002 and T003 can run in parallel after Phase 1 baseline confirmed:
Task: "Create core/src/types/edit.rs with Position/Range/TextEdit structs (T002)"
Task: "Bump BUS_PROTOCOL_VERSION constant in core/src/types/message.rs (T003)"

# After both land, T004 + T005 can run in parallel (different files):
Task: "Add EventPayload::BufferEdit variant in core/src/types/event.rs (T004)"
Task: "Update handshake_tests in core/src/bus/listener.rs (T005)"
```

## Parallel Example: Phase 3 (User Story 1)

```bash
# After Phase 2 complete, US1 implementation work parallelisable:
Task: "Add BufferState::apply_edits + ApplyError in buffers/src/model.rs (T007)"
Task: "Add weaver edit subcommand grammar in core/src/cli/args.rs (T011)"
Task: "Add range argv parser in core/src/cli/edit.rs (T012)"
Task: "Add WEAVER-EDIT-001/002 miette codes in core/src/cli/errors.rs (T014)"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (baseline confirm).
2. Complete Phase 2: Foundational (wire surface — CRITICAL, blocks all stories).
3. Complete Phase 3: User Story 1.
4. **STOP and VALIDATE**: Run `tests/e2e/buffer_edit_single.rs` and `tests/e2e/buffer_edit_inspect_why.rs`; operator runs Scenario 1 from quickstart manually.
5. MVP shippable here — slice 004 could end at this point with a reduced PR if the operator wants to defer US2/US3 to a follow-up slice. (Not the current plan; just noting feasibility.)

### Incremental Delivery (current plan)

1. Complete Setup + Foundational → wire surface ready.
2. Complete US1 → MVP single-edit lands; demo via Scenario 1.
3. Complete US2 → Atomic batch lands; demo via Scenario 2.
4. Complete US3 → JSON input lands; demo via Scenario 6.
5. Complete Polish → SC-403 + SC-404 e2e + CHANGELOG + operator-gates.
6. Run `scripts/ci.sh` green; PR-ready.

Each story adds value; none breaks the prior. Multi-session implementation per L2 / handoff-convention is expected — Phase 2 + most of Phase 3 plausibly fits one session; US2 + US3 + Polish a second session; quickstart walkthrough + final gate a third (or merged).

### Multi-session split

Per the slice-003 handoff convention (multi-session implementation when >40 tasks, >10-file commit clusters, or PR-discipline strain), slice 004 at 29 tasks COULD fit a single session. Realistically:

- **Session 1**: T001 + Phase 2 (T002–T006) + Phase 3 (T007–T016). End-of-session checkpoint after T016 e2e green. ~16 tasks landed.
- **Session 2**: Phase 4 (T017–T018) + Phase 5 (T019–T022) + Phase 6 (T023–T026). End-of-session checkpoint after T026. ~10 tasks landed.
- **Session 3 (operator-validated)**: T027 (operator quickstart walkthrough) + T028 (operator wall-clock judgment) + T029 (final CI gate + PR opening). 3 tasks landed; operator-gated.

Adjust based on observed scope drift mid-session.

---

## Operator-Involvement Map (STOP-AND-SURFACE)

Per slice-002/003 convention, the agent MUST stop and surface to the operator before reporting these tasks complete:

- **T015** (e2e single-edit) — wall-clock measurement is reported to stderr; the structural pass/fail is asserted by the test, but the SC-401 ≤500 ms judgment is operator-mode (T028).
- **T024** (e2e stale-drop) — race-window timing is hardware-dependent; observed wall-clock reported to stderr informationally.
- **T027** (quickstart walkthrough) — OPERATOR-REQUIRED by design.
- **T028** (SC-401 wall-clock) — OPERATOR-WALL-CLOCK; agent reports observed timing, operator judges.
- **T029** (final CI + PR readiness) — agent runs CI; on green, agent surfaces "ready for PR" and AWAITS operator confirmation before opening the PR.

---

## Notes

- Tasks-document convention: `[P]` parallelisable, `[USn]` story label (Phase 3+ only), `{surface:*}`/`{latency:*}` Weaver markers per L2 P7/P8/P18. No `{retraction}` / `{schema-migration}` / `{host-primitive}` markers used this slice (justified above).
- File-paths: absolute under repo root for `core/src/...`, `buffers/src/...`, `tests/e2e/...`.
- Tests are explicit (not optional) for slice 004: SC-401..406 are committed contracts; L2 P9 mandates scenario + property tests; slice-003 cadence carries over.
- Conventional Commits: `feat(bus)!:` for protocol bumps with `BREAKING CHANGE:` footer; `feat(buffers):` for service-side changes; `feat(cli):` for `weaver` subcommand additions; `test(buffers):` / `test(core):` for test additions; `docs(changelog):` and `docs(tasks):` for pure-docs.
- PR-discipline: one logical change per commit; `scripts/ci.sh` green at every commit; pre-commit hook installed; **single PR per slice** opened only after T029 + operator-gates green.
