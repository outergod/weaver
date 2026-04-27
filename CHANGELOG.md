# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/).
Per-public-surface versioning is per L2 constitution Principle 8 (`.specify/memory/constitution.md`).

## Public surfaces tracked

Per L2 Principle 7, each public surface carries its own version.

- **Bus protocol** v0.4.0 (was v0.3.0) — message categories, delivery classes (lossy / authoritative), CBOR tag scheme entries 1000 (entity-ref), 1001 (keyword), 1002 (structured actor identity). Slice 004 wire changes: `EventPayload::BufferEdit { entity, version, edits }` added; `TextEdit` / `Range` / `Position` struct types added (no new CBOR tag); `EventSubscribePattern` + `BusMessage::SubscribeEvents` added; `BusMessage::EventInspectRequest` / `EventInspectResponse` + `EventInspectionError` added; `InspectionDetail.value: FactValue` added as REQUIRED. `Hello.protocol_version` advances `0x03 → 0x04`. See `specs/004-buffer-edit/contracts/bus-messages.md`.
- **Fact-family schema `buffer/dirty`** v0.1.0 — wire shape unchanged from slice 001. **Authority transferred** from the `core/dirty-tracking` behavior to the `weaver-buffers` service in slice 003; the shipped core no longer registers an embedded producer.
- **Fact-family schema `buffer/path`** v0.1.0 (slice 003, new) — `FactValue::String` (canonical absolute path). Bootstrap fact asserted once per opened buffer; never updated (path change ≡ entity change). Authored by `weaver-buffers`.
- **Fact-family schema `buffer/byte-size`** v0.1.0 (slice 003, new) — `FactValue::U64` (the new variant lands under the bus-protocol MAJOR bump). Byte count of the service's in-memory content. Authored by `weaver-buffers`.
- **Fact-family schema `buffer/observable`** v0.1.0 (slice 003, new) — `FactValue::Bool`. Per-buffer file observability; `false` during transient unreadability, `true` on recovery. Edge-triggered per slice-002 F21. Authored by `weaver-buffers`.
- **Fact-family schema `buffer/version`** v0.1.0 (slice 003 post-merge, new) — `FactValue::U64`. Per-buffer applied-edit counter; `0` at bootstrap; bumped by each accepted `EventPayload::BufferEdit` in slice 004+. Forward-compat scaffolding so slice 004 doesn't have to BREAKING-expand the bootstrap fact-family set when the edit-versioning handshake lands. Authored by `weaver-buffers`.
- **Fact-family schema `repo/dirty`** v0.1.0 (slice 002, new) — `FactValue::Bool`; asserted by `weaver-git-watcher` per Clarification Q5 (index-or-working-tree differs from HEAD; untracked-only is clean). See `specs/002-git-watcher-actor/data-model.md`.
- **Fact-family schema `repo/head-commit`** v0.1.0 (slice 002, new) — `FactValue::String` holding the lowercase hex-encoded object id from `gix::rev_parse_single("HEAD")` — 40 chars for SHA-1 repositories, 64 for SHA-256. Retracted in the `Unborn` state.
- **Fact-family schema `repo/state/on-branch`** v0.1.0 (slice 002, new) — `FactValue::String` (branch name). Asserted iff HEAD points at `refs/heads/<name>`.
- **Fact-family schema `repo/state/detached`** v0.1.0 (slice 002, new) — `FactValue::String` (detached HEAD commit SHA).
- **Fact-family schema `repo/state/unborn`** v0.1.0 (slice 002, new) — `FactValue::String` (intended branch name for an empty repository).
- **Fact-family schema `repo/observable`** v0.1.0 (slice 002, new) — `FactValue::Bool`. `false` during watcher-`Degraded`; flips `true` on recovery. Suppresses dirty rendering in the TUI when `false` per `contracts/cli-surfaces.md`.
- **Fact-family schema `repo/path`** v0.1.0 (slice 002, new) — `FactValue::String` (canonical working-tree root). The three `repo/state/*` attributes obey a mutex invariant: at most one asserted per repository entity at any trace prefix (`docs/07-open-questions.md §26`).
- **Fact-family schema `watcher/status`** v0.1.0 (slice 002, new) — `FactValue::String` mirroring `LifecycleSignal` (`started` / `ready` / `degraded` / …). Keyed by the watcher's per-invocation instance-UUID entity, not the repository.
- **CLI surface `weaver`** v0.3.0 (was v0.2.0) — MINOR additive bump. Slice 004 adds `weaver edit <PATH> [<RANGE> <TEXT>]*` and `weaver edit-json <PATH> --from <PATH-or-dash>` subcommands plus a `--why` flag on `weaver inspect`. No removals; existing subcommand surfaces unchanged. The `bus_protocol` JSON field advances `"0.3.0" → "0.4.0"` mechanically as a by-product of the protocol bump (constant-driven). New diagnostic codes `WEAVER-EDIT-001` through `WEAVER-EDIT-004`.
- **CLI surface `weaver-buffers`** v0.1.0 — shape unchanged; `--version` JSON field `bus_protocol` advances `"0.3.0" → "0.4.0"` as a by-product of the protocol bump (constant-driven, not a CLI-surface change).
- **CLI surface `weaver-git-watcher`** v0.1.0 — shape unchanged; `--version` JSON field `bus_protocol` advances `"0.3.0" → "0.4.0"` as a by-product of the protocol bump (constant-driven, not a CLI-surface change).
- **CLI surface `weaver-tui`** v0.1.0 — shape unchanged; `--version` JSON field `bus_protocol` advances `"0.3.0" → "0.4.0"`. The Buffers render section already subscribes to `buffer/*` (slice-003 implementation); accepted-edit re-emissions flow through transparently — no new keybindings, no new render regions.
- **Configuration schema** v0.1.0 — unchanged.

## [Unreleased] — slice 004 "Buffer Edit"

**Breaking bus-protocol change** — version advances `0.3.0 → 0.4.0`. Slice-003 clients cannot connect to a v0.4.0 core. CLI `weaver` surface bumps MINOR additive for the new `edit` / `edit-json` subcommands and the `--why` flag on `inspect`. No removals from any surface. The slice introduces in-memory text editing on top of slice 003's `weaver-buffers` content authority — `weaver edit` is the first non-service event-payload producer wired to the wire, and `ActorIdentity::User` (reserved at slice 002) gets its first production use.

### Changed — bus protocol (MAJOR)

- **`Hello.protocol_version`** advances `0x03 → 0x04`. Mismatched clients receive `Error { category: "version-mismatch", detail: "bus protocol 0x04 required; received 0x03" }` and connection close. Detail-string format is pinned by `specs/004-buffer-edit/contracts/bus-messages.md §Connection lifecycle`.
- **`EventPayload::BufferEdit { entity, version, edits }`** added. Wire variant tag `"buffer-edit"` per Amendment 5. Carries an atomic batch of `TextEdit`s for an opened buffer; the publisher applies the batch in descending-offset order, bumps `buffer/version` by 1, and re-emits `buffer/byte-size` / `buffer/version` / `buffer/dirty` with the BufferEdit event's id as `causal_parent`. Empty batch (`edits: []`) is a silent no-op (no version bump).
- **`TextEdit { range, new_text }`**, **`Range { start, end }`**, **`Position { line, character }`** struct types added under `core/src/types/edit.rs`. Plain CBOR/JSON struct serialisation (no CBOR tag); `new_text` ↔ JSON `new-text` via `#[serde(rename_all = "kebab-case")]`. `Position.character` is a UTF-8 BYTE offset within the line's content (LSP-default departure justified in spec §Assumptions; forward-compatible with LSP 3.17 `positionEncodings` negotiation).
- **`InspectionDetail.value: FactValue`** added as REQUIRED. The 0x04 handshake rejects mixed-version clients, so no compat shim is needed. Slice-004's `weaver edit` emitter consumes `value` to extract the current `buffer/version` for the `BufferEdit` envelope.
- **`EventSubscribePattern { PayloadType(String) }`** + **`BusMessage::SubscribeEvents(EventSubscribePattern)`** added. Wire shape `{"type":"subscribe-events","payload":{"type":"payload-type","pattern":"buffer-edit"}}`. Subscribers receive `BusMessage::Event` frames whose payload's `type_tag()` matches the pattern. Lossy delivery via unbounded mpsc fan-out from a `Dispatcher`-owned `EventSubscriptions` registry. See `specs/004-buffer-edit/research.md §13`.
- **`BusMessage::EventInspectRequest { request_id, event_id }`** + **`BusMessage::EventInspectResponse { request_id, result: Result<Event, EventInspectionError> }`** + **`EventInspectionError::EventNotFound`** added. Powers `weaver inspect --why`'s chain walk: a fact's `InspectionDetail.source_event` resolves via `TraceStore::find_event` to the originating `Event` envelope. See `specs/004-buffer-edit/research.md §14`.
- **`EventPayload::type_tag(&self) -> &'static str`** added — returns the kebab-case wire discriminant for event-pattern matching (`"buffer-edit"`, `"buffer-open"`).

### Added — Weaver CLI (MINOR additive)

- **`weaver edit <PATH> [<RANGE> <TEXT>]* [--socket <PATH>]`** — dispatches a single `EventPayload::BufferEdit` event with the variadic positional pairs translated into a `Vec<TextEdit>`. `<RANGE>` parses as `<sl>:<sc>-<el>:<ec>` (decimal `u32`, UTF-8 byte offsets). Pre-dispatch flow: canonicalise path → derive entity → connect → `InspectRequest(buffer/version)` → construct envelope with `Provenance { source: ActorIdentity::User, .. }` → dispatch + exit 0. Fire-and-forget per FR-012 — silent drops at the service (stale-version, validation-failure, unowned-entity) are NOT detectable from the CLI. Zero-pair invocation emits a warn-stderr "no edits provided; nothing dispatched" and exits 0 without dispatching (FR-014).
- **`weaver edit-json <PATH> --from <PATH-or-dash> [--socket <PATH>]`** — JSON-driven equivalent of `weaver edit`. `--from -` reads stdin; `--from <PATH>` reads a named file. Pre-dispatch ingest-frame size check (`MAX_EVENT_INGEST_FRAME` = `MAX_FRAME_SIZE` − `RESPONSE_WRAPPER_HEADROOM` = 65 280 bytes) rejects oversized batches before they reach the codec. The 256-byte headroom reserves space for the `BusMessage::EventInspectResponse` wrapper used by `weaver inspect --why`, so any event accepted at ingest can be returned via walkback without exceeding the 64 KiB wire frame.
- **`weaver inspect <fact-key> --why`** — chains a second bus round-trip after the existing fact-inspect: takes `InspectionDetail.source_event`, issues `EventInspectRequest`, renders a walkback JSON shape with `fact`, `fact_inspection`, and `event` blocks. The `event.provenance.source.type` field carries the kebab-case `ActorIdentity` discriminator (e.g., `"user"` for `ActorIdentity::User`). Exit code 2 on either `FactNotFound` or `EventNotFound` (mirrors slice-001's "expected miss" convention).
- **Diagnostic codes** added in `core/src/cli/errors.rs`: `WEAVER-EDIT-001` (buffer not opened — pre-dispatch inspect-lookup returned `FactNotFound`), `WEAVER-EDIT-002` (invalid `<RANGE>` grammar OR odd-count variadic pairs), `WEAVER-EDIT-003` (malformed edit-json input), `WEAVER-EDIT-004` (serialised BufferEdit exceeds the 65 280-byte ingest-frame limit). All exit 1.
- `bus_protocol` JSON field advances `"0.3.0" → "0.4.0"` in all four binaries' `--version` output via the `BUS_PROTOCOL_VERSION_STR` constant. Constant-driven; not a CLI-surface schema event.

### Added — buffers consumer

- **`BufferState::apply_edits(&mut self, edits: &[TextEdit]) -> Result<(), ApplyError>`** — atomic two-phase apply pipeline (validate then mutate) per `specs/004-buffer-edit/data-model.md §Validation rules`. Empty batch is a structural identity (FR-008); on `Err` the buffer is byte-identical pre/post; on `Ok` `memory_digest == sha256(content)` is preserved structurally. Apply order is descending-offset (LSP 3.17 convention).
- **`ApplyError` taxonomy**: 6 variants (`OutOfBounds`, `MidCodepointBoundary`, `IntraBatchOverlap`, `NothingEdit`, `SwappedEndpoints`, `InvalidUtf8`) plus a `BoundarySide` enum carried in `MidCodepointBoundary`. Each variant has a `reason()` returning the kebab-case string the publisher emits in its `tracing::debug!` `reason` field per FR-018.
- **`BufferRegistry`** refactored from `(Vec<BufferState>, HashSet<EntityRef>)` to `HashMap<EntityRef, BufferState> + HashMap<EntityRef, u64> versions`. Map keyset is the ownership marker; per-buffer version counter initialised to 0 at bootstrap, bumped by accepted edits.
- **`dispatch_buffer_edit`** + **`BufferEditOutcome`** (`Applied { entity, new_version, new_byte_size }`, `NotOwned`, `StaleVersion { current, emitted }`, `FutureVersion { current, emitted }`, `EmptyBatch`, `ValidationFailure(ApplyError)`) — pure-ish dispatch handler mirroring slice-003's `dispatch_buffer_open` shape. Reader-loop arm forwards each accepted batch's three re-emitted facts with `causal_parent = Some(event.id)`.
- **Event-broadcast subscription** at bootstrap: publisher subscribes to `EventSubscribePattern::PayloadType("buffer-edit")` immediately after handshake; reader-loop forwards `BusMessage::Event` frames into the dispatch arm.
- **`buffer_entity_ref`** (and `BUFFER_NAMESPACE_BIT` / `INSTANCE_NAMESPACE_BIT` / `REPO_NAMESPACE_BIT`) lifted from `weaver-buffers::model` to `weaver_core::types::buffer_entity` so the `weaver edit` CLI can derive the same entity-id without a circular cargo dep. `weaver-buffers::model` re-exports for slice-003 callers; hash output byte-identical pre/post.

### Added — `ActorIdentity::User`

- First production use of the `User` unit variant (reserved at slice 002 for human-initiated CLI actions). `weaver edit` and `weaver edit-json` stamp `Provenance { source: ActorIdentity::User, .. }` on dispatched `BufferEdit` events. The `User` variant became a unit (no fields) in this slice — the original placeholder `User { id }` shape was speculative; for a single-process local editor with one human operator there's no need to attribute edits across multiple users. `UserId` newtype removed entirely.

### Fixed — buffers (mid-flight)

- Reader-loop's `Applied` arm now syncs `state.last_dirty` to the published value, suppressing a poll-loop double-fire of `buffer/dirty=true` after each accepted edit. Detected by the T018 atomic-batch e2e proptest. The data-model's atomic-batch contract commits to ONE re-emission per accepted edit; the prior code's reader-loop arm published once, then the next 100 ms poll tick's edge-trigger fired a second time.

### Test summary for slice 004

- `apply_edits` property test (T017) — 256 cases pinning the R1..R6 + intra-batch-overlap iff oracle and the digest-invariant + atomicity postconditions.
- `buffer_edit_atomic_batch` e2e (T018) — 16-edit happy batch + 3-edit batch with invalid middle, asserting "exactly one re-emission burst" structurally.
- `buffer_edit_emitter_parity` proptest (T022) — 256 cases over a fake-core test harness asserting `weaver edit` and `weaver edit-json` dispatch byte-identical CBOR payloads.
- `buffer_edit_sequential` e2e (T023) — 100 sequential edits land cleanly with no gaps and no duplicates (SC-403 structural).
- `buffer_edit_stale_drop` e2e (T024) — barrier-coordinated two-emitter race, asserting the loser's dispatch drops at the publisher with `reason="stale-version"` in the captured stderr (SC-404).
- `buffer_edit_single` e2e (T015) — single-edit dispatch + buffer-not-opened CLI path (SC-401 + WEAVER-EDIT-001).
- `buffer_edit_inspect_why` e2e (T016) — `weaver inspect --why` walks back to the BufferEdit event with `event.provenance.source.type == "user"` (SC-405).

## [0.3.0a] — 2026-04-25 — slice-003 forward-compat tail (PR #10)

Pre-slice-004 forward-compat: ships the `buffer/version` fact family on the slice-003 bootstrap set so slice 004 can begin bumping the counter without expanding the fact family count in the same PR. Additive across every public surface; no bus-protocol bump (stays `0.3.0`).

### Added — buffer/version fact family

- **Fact-family schema `buffer/version`** v0.1.0 — `FactValue::U64`. `weaver-buffers` publishes `buffer/version=0` as a fifth bootstrap fact alongside `buffer/path` / `buffer/byte-size` / `buffer/dirty` / `buffer/observable`. No consumer in this change; no edit path in slice 003.
- Forward-compat motivation: slice 004 introduces `EventPayload::BufferEdit` with an edit-versioning handshake (stale edits referencing a pre-edit version are rejected server-side). Shipping the version field on the bootstrap set now means slice 004 can start bumping the counter on each accepted edit without expanding the bootstrap fact count in the same PR. The `buffers/tests/component_discipline.rs` proptest's attribute→type-map assertion already pins the shape.
- `buffer_bootstrap_facts` in `buffers/src/model.rs` now returns `[(&'static str, FactValue); 5]` (was `; 4`). `publish_buffer_bootstrap` still iterates via the seam, so the new fact flows to the wire automatically. Retract set in `tests/e2e/buffer_sigkill.rs` updated to the five attributes.

## [0.3.0] — 2026-04-24 — slice 003 "Buffer Service"

**Breaking bus-protocol change** — version advances `0.2.0 → 0.3.0`. Slice-002 clients cannot connect to a v0.3.0 core; every in-tree bus client (core, TUI, git-watcher, new `weaver-buffers`, e2e test harness) rebuilds together. CLI `weaver` surface bumps MAJOR for the `simulate-edit` / `simulate-clean` removal. New `weaver-buffers` binary ships at 0.1.0. The slice adds Weaver's first content-backed service and retires the slice-001 embedded `DirtyTrackingBehavior`, leaving `buffer/dirty` authored only by a process on the bus.

### Changed — bus protocol (MAJOR)

- **`Hello.protocol_version`** advances `0x02 → 0x03`. Mismatched clients receive `Error { category: "version-mismatch", detail: "bus protocol 0x03 required; received 0x02" }` and connection close. Detail-string format is pinned by `specs/003-buffer-service/contracts/bus-messages.md §Connection lifecycle`.
- **`EventPayload::BufferEdited`** and **`EventPayload::BufferCleaned`** removed. The `weaver simulate-edit` / `simulate-clean` subcommands that produced them are removed in lockstep.
- **`EventPayload::BufferOpen { path }`** added. Kebab-case variant tag `"buffer-open"` per Amendment 5. Slice 003 dispatches this event in-process from `weaver-buffers`'s bootstrap loop; slice 004+ will accept it over the wire from external producers.
- **`FactValue::U64(u64)`** added under the existing adjacent-tag `#[serde(tag = "type", content = "value", rename_all = "kebab-case")]`. Wire form `{"type":"u64","value":<n>}`. Carries `buffer/byte-size` where file sizes above `i64::MAX` would otherwise truncate.

### Removed — core (MAJOR)

- **`core/dirty-tracking` behavior** and its registration call in `core/src/cli/run.rs` are gone. `buffer/dirty` is no longer produced in-core; the family's authority transfers to `weaver-buffers` (service-only). The `DirtyTrackingBehavior` test fixture migration to a payload-agnostic `StubBehavior` in `core/tests/common/mod.rs` keeps the slice-002 inspection + property tests intact.
- **`weaver simulate-edit`** / **`simulate-clean`** CLI subcommands removed. Clap exits code 2 with `"unrecognized subcommand"` for either invocation.
- **TUI `e` / `c` keystrokes** removed (their events no longer exist on the wire); command bar now renders `Commands: [i]nspect  [q]uit`. Keybinding removal is a MAJOR under `cli-surfaces.md §Versioning policy`, bundled here with the simultaneous wire-variant removal.

### Added — buffers crate scaffold

- New workspace member `buffers/` — produces the `weaver-buffers` binary. `Cargo.toml` inherits `tokio`, `clap` (`derive`), `miette`, `thiserror`, `tracing`, `tracing-subscriber`, `uuid`, `sha2`, `humantime`, `serde`, `ciborium`, and a path dep on `weaver-core`; dev-deps inherit `tempfile` + `proptest`.
- Workspace dep: `sha2 = "0.10"` (new) for SHA-256 digests.
- `buffer_entity_ref(&Path) -> EntityRef` derives a stable entity with reserved bit 61 (buffer namespace) and bits 62 / 63 cleared — distinct from slice-002's watcher-instance (bit 62) and slice-001's repo (bit 63) namespaces. Trace inspection classifies an entity's namespace at a glance by the bit layout.
- `watcher_instance_entity_ref(&Uuid) -> EntityRef` mirrors slice-002's derivation: bit 62 set, bit 63 cleared. The two services share the watcher-instance namespace; the TUI/inspect machinery distinguishes them by asserted facts, not bit layout.
- `BufferState` with private fields (`path`, `entity`, `content`, `memory_digest`, `last_dirty`, `last_observable`); the fallible `open()` constructor establishes the `memory_digest == Sha256(content)` invariant structurally. A custom `Debug` impl redacts `content` so accidental `tracing::debug!(?state)` never emits file bytes (FR-002a).
- `BufferObservation` / `ObserverError` / `StartupKind` typed outputs for the observer path; `StartupKind` selects the CLI's `WEAVER-BUF-00{1,2,3}` miette diagnostic code.

### Added — Phase 3: `weaver-buffers` end-to-end (US1)

- **Observer** (`buffers/src/observer.rs`): streams the on-disk file through a SHA-256 hasher, compares `disk_digest` to `state.memory_digest`, emits a `BufferObservation { byte_size, dirty, observable: true }`. `std::io::ErrorKind::NotFound` maps to `Missing`; a successful metadata with `!is_file()` maps to `NotRegularFile`; other I/O errors map to `TransientRead`.
- **Publisher** (`buffers/src/publisher.rs`): one `weaver-buffers` process asserts authority over the `buffer/*` families for the opened buffer set via `ActorIdentity::Service { service_id: "weaver-buffers", instance_id }` with a fresh UUID v4 per invocation. Stream splits post-handshake; a reader task drains server-sent `Error` frames (`authority-conflict` → exit 3, `not-owner` → soft AuthorityConflict with prefix → exit 3, other → exit 10). Slice-002 F31 follow-up remains out of scope per `research.md §9`.
- **Bootstrap sequence** (C11): `watcher/status=started` (causal_parent=None) → per-buffer 4-fact bootstrap (`buffer/path`, `buffer/byte-size`, `buffer/dirty=false`, `buffer/observable=true`, each carrying a per-buffer synthesised `EventId` as `causal_parent`) → `watcher/status=ready`. Fail-fast on any open failure: partial-retract every fact already asserted, emit a miette diagnostic on stderr, exit 1.
- **Poll loop** (C12, default 250 ms interval): sequential per-buffer iteration via `observer::observe_buffer`; edge-triggered `buffer/dirty` (republish only when the flag flips) and `buffer/observable` (false on first failed observation, true on recovery). Service-level `watcher/status=degraded` fires only when every currently-open buffer is simultaneously unobservable (FR-016a); recovery republishes `ready`.
- **Shutdown** (C13): on SIGTERM / SIGINT, retract every tracked `buffer/*` fact, then overwrite `watcher/status` with `unavailable` → `stopped`, abort the reader task, exit 0. On reader-loop EOF (core gone), no retract attempt (`PublisherError::BusUnavailable` → exit 2); core's `release_connection` covers cleanup server-side.
- **CLI surface (new binary)** — `weaver-buffers <PATH>... [--poll-interval=250ms] [--socket=<path>] [--output=human|json] [-v/-vv/-vvv] [--version]`. `--socket` folds `WEAVER_SOCKET` (parity with `weaver`, `weaver-git-watcher`). `--poll-interval=0ms` is rejected at parse time. Documented exit codes: 0 clean, 1 startup failure, 2 bus unavailable, 3 authority conflict, 10 internal. Startup failures render WEAVER-BUF-001 (not openable), WEAVER-BUF-002 (not a regular file), WEAVER-BUF-003 (too large, currently `std::io::ErrorKind::OutOfMemory`), WEAVER-BUF-004 (authority conflict) miette diagnostics.

### Added — Phase 3: TUI Buffers render section

- `tui/src/render.rs` renders a **Buffers** section below the existing Repositories section — one row per buffer entity, showing `<path> [<bytes> bytes] <dirty-badge>` plus the authoring-actor line `by service weaver-buffers (inst <short-uuid>), event <id>, <t>s ago`. The `[observability lost]` badge replaces the dirty indicator when `buffer/observable = false`; `[stale]` is appended per-row when the TUI loses its core subscription. Row ordering is deterministic by `(entity, attribute)` per slice 002 convention.
- `FamilyPrefix("buffer/")` added to the TUI's subscription set in `tui/src/client.rs`.

### Added — Phase 4: authority handoff (US2)

- `weaver inspect <buffer-entity>:buffer/dirty` attributes the fact to `weaver-buffers` (`asserting_kind = "service"`, `asserting_service = "weaver-buffers"`, `asserting_instance = <uuid>`, `asserting_behavior = absent`). e2e coverage in `tests/e2e/buffer_inspect_attribution.rs` (SC-305).
- Cross-authority overwrite scenario (`core/tests/inspect/buffer_behavior_service_overwrite.rs`): a behavior-authored `buffer/dirty=true` injected into the fact store is overwritten by a service-authored `buffer/dirty=false` via the bus; `weaver inspect` returns the service's provenance, not the behavior's. Exercises FR-013 + the slice-002 F23 live-fact-provenance invariant through the slice-003 authority boundary.
- Slice-001 e2e tests transformed onto `weaver-buffers`: `hello_fact.rs` retains its structural smoke shape (assert-post-bootstrap + SIGTERM-retracts, no latency assertions — the per-SC tests own those budgets); `disconnect.rs` flips to core-killed → service-exits (exit code 2 pins T038's bus-EOF classification); `subscribe_snapshot.rs` verifies late-subscriber snapshot replay for service-authored facts with `weaver-buffers` attribution.
- `tests/e2e/buffer_simulate_removed.rs` pins the CLI-level removal of `weaver simulate-edit` / `simulate-clean` — exit code 2 with `"unrecognized subcommand"` in stderr.

### Added — Phase 5: multi-buffer within one invocation (US3)

- CLI-level path canonicalization + dedup at parse time (FR-006a). `buffers/src/cli.rs::canonicalise_and_dedup` collapses duplicate expressions of the same path (`./foo.txt`, `foo.txt`) into one unique-canonical set; a `debug!()` log fires when the dedup count differs. Incidentally fixed a pre-slice-003 latent bug where `./foo.txt` and `foo.txt` would have derived distinct buffer entities had the service ever been run with argv-as-typed (which slice 003 never does).
- Defensive `BufferOpen` idempotence at the publisher dispatch layer: `BufferRegistry` + `pub(crate) fn dispatch_buffer_open` + `BufferOpenOutcome::{Fresh, AlreadyOwned}`. CLI hot path never triggers `AlreadyOwned` under slice-003 argv (T055 dedups upstream); slice-004+ wire producers that emit `BufferOpen` over the bus will exercise the branch. The seam is `pub(crate)`, not `pub` — slice 004 threads the handler into its `reader_loop` arm.
- Authority-conflict enforcement: a second `weaver-buffers` instance launched on an overlapping path receives `Error { category: "authority-conflict" }` from core, maps to `PublisherError::AuthorityConflict`, exits code 3 within 1 s (SC-304). The first instance's facts remain unperturbed. e2e coverage in `tests/e2e/buffer_authority_conflict.rs` pins exit code 3 the same way `tests/e2e/disconnect.rs` pins exit code 2.
- Per-buffer `buffer/observable` is edge-triggered per buffer; service-level `watcher/status=degraded` is the aggregate — `ready` stays asserted while any buffer remains observable. Recovery (file restored) re-publishes `buffer/observable=true` for the restored entity and `watcher/status=ready` if the service had flipped to `degraded`.

### Added — Phase 6 test coverage

- e2e: `buffer_{open_bootstrap, external_mutation, sigkill}.rs` — SC-301 (≤1 s cold start), SC-302 (≤500 ms external mutation), SC-303 (≤5 s SIGKILL retract).
- e2e: `buffer_{inspect_attribution, simulate_removed}.rs` + `core/tests/inspect/buffer_behavior_service_overwrite.rs` — SC-305 + F23.
- e2e: `buffer_{multi_buffer, authority_conflict, degraded_observable}.rs` — US3 scenarios.
- Property tests: `buffers/tests/component_discipline.rs` (T062, SC-306) pins the attribute→`FactValue` type map through a `pub fn buffer_bootstrap_facts` seam; `buffers/tests/path_canonicalization.rs` (T064) exercises `buffer_entity_ref` determinism + reserved-bit invariants + canonicalize idempotence; `core/tests/property/factvalue_u64_roundtrip.rs` (T063) lifts the new variant's JSON + CBOR round-trip to the full `u64` domain.

### Migration notes

- Slice-001 e2e tests were **transformed**, not retired. Every slice-001 scenario that named `weaver publish` / `simulate-edit` / `simulate-clean` now drives `weaver-buffers` with a `tempfile::TempDir` fixture + `std::fs::write` to flip dirty. The `#[ignore]` gates added during slice 003 Phase 2 are all removed as of Phase 4.
- Third-party bus clients upgrading to v0.3.0 must: (a) rebuild against the new `BUS_PROTOCOL_VERSION` constant, (b) drop any code that sends or handles `BufferEdited` / `BufferCleaned`, (c) handle `BufferOpen { path }` on the receive path if they consume events, (d) accept `FactValue::U64` as a valid value variant. The protocol-version-mismatch path logs the required version verbatim for operator diagnosis.

## [0.2.0] — 2026-04-23 — slice 002 "Git-Watcher Actor"

**Breaking bus-protocol change** — version bumps `0.1.0 → 0.2.0`. Slice 001 clients cannot connect to a v0.2.0 core; all in-tree bus clients (core, TUI, CLI, e2e test harness, test client) rebuild together.

### Changed — bus protocol (MAJOR)

- **Provenance `source` shape** changed from opaque `SourceId::External(String)` to structured `ActorIdentity` — one closed enum per actor kind in `docs/01-system-model.md §6`. Variants: `Core`, `Behavior { id }`, `Tui`, `Service { service-id, instance-id }`, `User { id }`, `Host { host-id, hosted-origin }`, `Agent { agent-id, on-behalf-of }`. Wire shape: internally-tagged CBOR/JSON with kebab-case `type` discriminator and kebab-case field names. Closes `docs/07-open-questions.md §25` sub-questions: *shape* (single closed enum) and *migration* (replace, not extend). See `specs/002-git-watcher-actor/` Clarifications Q1, Q2.
- **New CBOR tag 1002** reserved for structured actor identity (adjacent to the slice-001 tags 1000 and 1001).
- **`LifecycleSignal`** extended with `Degraded`, `Unavailable`, `Restarting` variants per `docs/05-protocols.md §5`. Slice-001 core continues to emit only `Started` / `Ready` / `Stopped`; the richer states are intended for services that can degrade without exiting.
- **`Hello.protocol_version`** advances `0x01 → 0x02`. Mismatched clients receive `Error { category: "version-mismatch", ... }` and connection close (unchanged handshake logic; bumped constant).

### Added — core

- `ActorIdentity::service(service_id, instance_id)` constructor with kebab-case validation (L2 Amendment 5); rejects empty identifiers and identifiers containing uppercase, underscores, whitespace, leading/trailing/consecutive hyphens.
- `ActorIdentity::behavior(id)` / `ActorIdentity::user(id)` convenience constructors.
- `UserId`, `HostedOrigin` placeholder types — reserved for forward-compat, not emitted this slice.
- `kind_label()` method on `ActorIdentity` for diagnostic rendering.
- `uuid` workspace dependency (`v4` feature) per Clarification Q3.

### Added — watcher crate scaffold

- New workspace member `git-watcher/` — produces the `weaver-git-watcher` binary. Phase 1 scaffold only: CLI prints a Phase-1 marker and exits. Real implementation lands in Phase 3 (US1).
- Workspace deps: `gix = "0.66"` (pure-Rust git; research §1), `humantime = "2"` (for `--poll-interval` in Phase 3).

### Added — Phase 3: `weaver-git-watcher` end-to-end (US1)

- **Observer**: `RepoObserver` opens a repository via `gix::discover`, keys the watcher by the **discovered working-tree root** (never the user-typed input path — prevents two watchers on different subpaths from bypassing the authority mutex). Bare repositories are rejected at `open()` with a dedicated `BareRepositoryUnsupported` variant; in-progress transient operations (rebase / merge / cherry-pick / revert / bisect) surface as `UnsupportedTransientState` so the watcher flips to `Degraded` rather than misreporting branch state. Symbolic HEAD outside `refs/heads/` (e.g. pointing at a tag) surfaces as `UnsupportedHeadShape`. HEAD-resolve failures on an `OnBranch` / `Detached` state propagate as `ObserverError::Observation`. Dirty check uses `git diff HEAD --quiet` via shell-out (documented deviation from research §1); SHA resolution uses `gix`.
- **Publisher**: one `weaver-git-watcher` process asserts authority over `repo/*` for a single repo entity via an `ActorIdentity::Service` with a fresh UUID v4 per invocation (Clarification Q3). The publisher splits its bus stream post-handshake so a reader task can surface server-sent `Error` frames (`authority-conflict`, `identity-drift`, `not-owner`, `invalid-identity`) to the main loop, exiting with the documented code path (`2` bus-unavailable, `3` authority-conflict, `10` internal). Degraded-state emission is **edge-triggered**: the `Lifecycle(Degraded)` + `repo/observable=false` pair fires only on the healthy→degraded transition, not every failed poll.
- **Authority-conflict mechanism** (core): new `AuthorityMap` + `ServicePublishOutcome` + `ServiceRetractOutcome` in `core/src/behavior/dispatcher.rs`. Claims are **conn-keyed** (identity alone is client-forgeable on the wire) and a connection binds its `ActorIdentity` on first publish — any subsequent publish under a different identity returns `ServicePublishOutcome::IdentityDrift`, surfaced over the bus as `Error { category: "identity-drift" }`. Retract attribution is synthesized server-side (client-supplied `source` and `timestamp_ns` are ignored; only `causal_parent` survives as a correlation hint).
- **Connection-owned fact tracking**: every service-asserted fact is recorded against its owning connection; `release_connection` retracts everything the connection published when the stream closes, so SIGKILL of a watcher leaves no stale `repo/*` facts in the store.
- **CLI surface (new binary)** — `weaver-git-watcher <REPOSITORY-PATH> [--poll-interval=250ms] [--socket=<path>] [--output=json|human] [-v/-vv/-vvv] [--version]`. `--socket` folds `WEAVER_SOCKET` env var (parity with `weaver`). `--output=json` switches both `--version` rendering AND runtime tracing to JSON. `--poll-interval=0ms` is rejected at parse time (would panic `tokio::time::interval`). Documented exit codes: 0 clean, 1 startup failure (including bootstrap `observe()` errors), 2 bus unavailable, 3 authority conflict, 10 internal.

### Added — Phase 3: TUI Repositories section

- `tui/src/render.rs` renders a dedicated **Repositories** section below the existing Facts section. State badges: `[on <name>]`, `[detached <sha>]`, `[unborn <name>]`, or `[state unknown]`. The `[observability lost]` badge replaces the dirty indicator when `repo/observable = false`; `[stale]` is appended per-row when the TUI loses its core subscription. Authoring-actor line reuses the shared `annotation` helper to render `by service <id> (inst <short-uuid>), event <id>, <t>s ago`. Facts and Repositories sections both order facts deterministically by `(entity, attribute)` so `[i]nspect` always targets the visually-first fact.

### Added — Phase 4: structured-identity inspection

- `InspectionDetail` gains an always-present `asserting_kind: String` discriminator — `"behavior" | "service" | "core" | "tui" | "user" | "host" | "agent"` (see `ActorIdentity::kind_label`). Identifier fields are populated only for the slice's emitted kinds (`behavior`, `service`). Core / Tui / reserved variants carry the kind alone. Additive per cli-surfaces.md §wire compatibility.
- Backward-compatible deserialization via `InspectionDetailRepr` + `#[serde(from = ...)]`: mixed-version deployments continue to work — a new client decoding a pre-slice response infers `asserting_kind` from the populated identifier fields (`behavior` / `service` / fallback `core`).
- Inspection routes through the **live fact's provenance**, not the `TraceStore::fact_inspection` index, so a service overwriting a behavior-authored fact is now attributed correctly (the behavior index isn't cleared on overwrite — only on retraction — and the inspect handler no longer relies on it for authoritative attribution).

### Added — Phase 4: wire-edge identity validation

- `ActorIdentity::validate()` is the **single gate for wire-derived provenance**. Called from `Provenance::new` (in-process safety) and listener-side for both `BusMessage::FactAssert` and `BusMessage::Event`. Rejects empty `service-id`, `behavior-id`, `user-id`, `host-id`, `hosted-origin.{file,runtime-version}`, `agent-id`; recursively validates `Agent.on_behalf_of`. `Service` identifiers additionally must be kebab-case (Amendment 5). Malformed wire frames receive `Error { category: "invalid-identity" }`. Non-`Service` provenance on `FactAssert` is rejected with `Error { category: "unauthorized" }` (behaviors publish in-core; only services publish over the bus).

### Changed — bus dispatcher

- Lock-order across the dispatcher standardized: `publish_from_service` and `retract_from_service` now both acquire `fact_store` before `conn_facts`; the retract path releases the `conn_facts` guard before awaiting `fact_store.lock()` (the inverse held prior and admitted a deadlock under concurrent publish + non-owner retract traffic).
- `listener.rs::handle_connection` funnels every post-handshake exit through a single `dispatcher.release_connection(conn_id)` call — a forwarding-write failure on a publisher-subscriber connection no longer leaks authority claims or conn-owned facts.

### Fixed — miscellaneous polish

- `weaver-git-watcher --version` honours `--output=json|human` per the CLI contract; three binaries (`weaver`, `weaver-git-watcher`, `weaver-tui`) all report `bus_protocol: "0.2.0"` from the same constant.
- TUI `short_sha` truncation is UTF-8 safe (char-iterator-based); the repo-view representative fact uses the freshest `asserted_at_wall_ns` so the rendered age reflects the watcher's most recent publication, not a startup-only one.

### Added — Phase 3 & 4 test coverage

- `git-watcher/tests/mutex_invariant.rs` (T060) — property test over 1–20-observation random sequences proves the discriminated-union `repo/state/*` mutex invariant holds at every trace prefix.
- `git-watcher/tests/transition_causal.rs` (T061) — six scenario tests exhausting the variant-pair matrix; retract and assert of every transition share a `causal_parent` EventId equal to the triggering poll tick.
- `core/tests/inspect/behavior_authored.rs` (T065), `tests/e2e/git_watcher_inspect.rs` (T066), `core/tests/inspect/structured_always.rs` (T067), `core/tests/inspect/causal_walkback.rs` (T067a), `core/tests/property/inspect_identity.rs` (T068) — Phase-4 inspection coverage: CLI-level attribution for behavior- and service-authored facts, structured-always invariant across fact families, multi-hop causal-chain identity check, round-trip property for every emitted kind.
- `tests/e2e/{git_watcher, git_watcher_sigkill, git_watcher_authority_conflict, fact_assert_identity_guard}.rs` — end-to-end three-process coverage.

## [0.1.0] — 2026-04-20 — slice 001 "Hello, fact"

Initial public release. Ships the end-to-end skeleton: core + TUI +
one embedded behavior + bus + fact space + trace + inspection + CLI,
together validating L2 P1/P2/P4/P5/P6/P9/P10/P11/P12/P13/P19/P20.

All four public surfaces (bus protocol, `buffer/dirty` fact schema,
CLI, configuration) debut at their initial versions. Spec success
criteria SC-001 through SC-006 are met and covered by automated
tests (54 unit + 13 integration + 2 e2e).

### Added — slice 001 "Hello, fact"

Entries are organised by phase of `specs/001-hello-fact/tasks.md`.

#### Phase 1 — Setup

- Workspace `Cargo.toml` with `[workspace.package]` (edition 2024, rust-version 1.85, license `AGPL-3.0-or-later` matching `LICENSE`) and `[workspace.dependencies]` for tokio, serde + serde_json, ciborium, clap, miette + thiserror, tracing, proptest, vergen, crossterm. The initial scaffold incorrectly defaulted the license to `MIT OR Apache-2.0` (Rust-ecosystem default); aligned to AGPL per L2 Amendment 4.
- `rust-toolchain.toml` pinning the stable Rust channel for reproducible builds (L2 P19).
- `.gitignore` Rust patterns (`target/`, `**/*.rs.bk`, `*.sock`) with explicit guidance to keep `Cargo.lock` tracked (L2 P19).
- `core` crate scaffold with `[lib]` (`weaver_core`) and `[[bin]]` (`weaver`) targets and `build.rs` invoking `vergen` for build-time provenance (L2 P11).
- `tui` crate scaffold (`weaver-tui` binary) depending on `weaver_core` for shared types.
- `ui` crate stub (Tauri UI deferred per Hello-fact slice 001 scope).

#### Phase 3 — User Story 1 (MVP: trigger + propagate)

- **Fact-family `buffer/dirty` (v0.1.0)**: first live producer. `core/dirty-tracking` behavior asserts `buffer/dirty=true` on `buffer/edited` events and retracts it on `buffer/cleaned`.
- **Bus protocol (v0.1.0)**: subscriptions now *forward* `FactAssert`/`FactRetract` messages to subscribers in real time. The Phase 2 listener acked subscriptions but never forwarded; the new listener multiplexes client reads and subscription fan-out via `tokio::select!`. No wire-format change — the behavior completes what v0.1.0 always promised.
- **CLI surface (v0.1.0)**:
  - `weaver simulate-edit <buffer-id>` now publishes `buffer/edited` on the bus (previous Phase 2 stub was a warn log).
  - `weaver simulate-clean <buffer-id>` now publishes `buffer/cleaned` (previous Phase 2 stub).
  - Both commands return a structured submission ack in `--output=human` or `--output=json`.
- **TUI**: crossterm raw-mode event loop with `e`/`c`/`q` keystrokes; live rendering of subscribed facts with `by <behavior>, event <id>, Δs ago` annotation; stale-view rendering with `UNAVAILABLE` status on core disconnect per `contracts/cli-surfaces.md`.
- **Dispatcher**: commit is now atomic with respect to behavior error — when a behavior firing returns `error: Some(_)`, its assertions and retractions are rolled back and the `BehaviorFired` trace entry records empty `asserted`/`retracted` lists. Tightens the implicit contract that Phase 2's docstring already claimed; covered by the new `error_recovery` scenario test.
- **Shared bus-client helper (`core/src/bus/client.rs`)**: consolidates the `Hello`/`Lifecycle(Ready)`/`Subscribe` handshake used by the CLI's one-shot subcommands, the TUI, and the e2e harness. Consolidation paves the way for the inspect client in Phase 4.
- **Workspace member `weaver-e2e`** (`tests/`): workspace-level end-to-end tests spawning the `weaver` binary. Two tests ship with this phase: `hello_fact` (SC-001, happy + retraction round-trip ≤ 100 ms) and `disconnect` (SC-004, SIGKILL survivability within 5 s).

#### Phase 4 — User Story 2 (provenance inspection)

- **Bus protocol (v0.1.0)**: `InspectRequest` now returns a real `InspectionDetail` instead of an always-`FactNotFound` placeholder. The handler walks the trace store's reverse causal index (already built in Phase 2) and is `O(1)` per lookup.
- **CLI surface (v0.1.0)**:
  - `weaver inspect <entity-id>:<attribute>` is now live. Parses the colon-delimited fact key, issues `InspectRequest`, renders human or JSON output matching `contracts/cli-surfaces.md`. Exit code 2 on `FactNotFound`.
  - Input validation — malformed keys (missing colon, empty halves, non-numeric entity id) produce structured errors before touching the bus.
- **TUI**: `i` keystroke triggers inspection of the first displayed fact. Waiting state rendered explicitly (`(waiting for response…)`) between request send and response; correlation via `request_id` so out-of-order InspectResponses are handled safely.
- **`core/src/inspect/handler.rs`** (new): pure routine `inspect_fact(snapshot, trace, key) -> Result<InspectionDetail, InspectionError>`. Uses `FactSpaceSnapshot` for the current-assertion check (fast `Arc` clone) and `TraceStore::fact_inspection` for the asserting behavior/event lookup.
- **New test coverage**: `inspect_inspection_found`, `inspect_inspection_not_found`, `property_inspection_invariant`, plus fact-key-parser unit tests in `cli::inspect::tests`.

#### Phase 5 — User Story 3 (structured machine output)

- **Bus protocol (v0.1.0 — additive)**: two new `BusMessage` variants — `StatusRequest` (client → core, unit) and `StatusResponse { lifecycle, uptime_ns, facts }` (core → client). Additive surface change per L2 P7. A future slice with a deployed v0.1 client will bump `Hello.protocol_version` if a wire-incompatible change ships; the protocol-level CBOR deserializer does NOT yet handle unknown variants gracefully, so adding variants *today* is only safe because all clients are co-developed in this repo. This caveat is a known gap in the contract — to be tightened in a future slice.
- **CLI surface (v0.1.0)**:
  - `weaver status [-o human|json]` is now live (was a warn-log stub). Connects to the bus, sends `StatusRequest`, renders the response per `contracts/cli-surfaces.md`.
  - On `core-unavailable`: renders the documented `{"lifecycle": "unavailable", "error": "..."}` shape and exits `2`.
  - Exit-code policy centralised in `cli::errors::exit_code` (`OK=0`, `GENERAL=1`, `EXPECTED=2`).
- **Error surface (new)**: `WeaverCliError` in `core/src/cli/errors.rs` with `miette::Diagnostic` derive. Four codes wired up (`WEAVER-002` core-unavailable, `WEAVER-101` parse-error, `WEAVER-201` fact-not-found, `WEAVER-301` protocol-error). JSON envelope matches contract: `{ "error": { "category", "code", "message", "context", "fact_key" } }`. `fact_key` populated for fact-scoped errors per L2 P6.
- **Dispatcher**: tracks `started_at_ns`; exposes `Dispatcher::uptime_ns()` for the status handler.
- **Listener**: handles `StatusRequest` by snapshotting the fact-store, reading dispatcher uptime, and replying with `StatusResponse`.
- **`core/src/cli/output.rs`** (new): `StatusResponse` struct with serde round-trip, `render_status` dispatcher, human and JSON formatters. Unit tests verify (a) round-trip preservation, (b) ready shape omits `error`, (c) unavailable shape omits `uptime_ns` and `facts`.
- **New test coverage**: `cli_status_round_trip`, `cli_status_unavailable` (exit-code 2), `cli_status_human`, plus `cli::output::tests` (4) and `cli::errors::tests` (3).

#### Phase 5.5 — Wire-tagging alignment (pre-1.0 cleanup)

The first slice is the right time to unify serialization strategy across
public wire surfaces. Adopted **adjacent tagging** (`"type"` + variant-specific
content field) uniformly for every sum type with non-unit variants:

- **`SourceId`** — `#[serde(tag = "type", content = "id")]`. Wire form now
  matches `contracts/cli-surfaces.md`'s example literally:
  `{"type":"behavior","id":"core/dirty-tracking"}` (was `{"behavior":"..."}`).
- **`BusMessage`** — `#[serde(tag = "type", content = "payload")]`. Every
  message variant now shares the shape `{"type":"<kind>","payload":<data>}`
  (unit variants omit `payload`). Uniform `.type`-based dispatch for
  consumers (was a rotating outer key per variant).
- **`SubscribePattern`** — `#[serde(tag = "type", content = "pattern")]`.
  `{"type":"family-prefix","pattern":"buffer/"}` (was `{"family-prefix":"..."}`).

`FactValue` already used adjacent tagging (`tag="type", content="value"`) — all
four data-bearing enums now share the pattern. Unit-only enums
(`LifecycleSignal`, `EventPayload`, `InspectionError`) remain bare kebab-case
strings, which is naturally consistent with adjacent-tag content semantics.

**Why now and not later**: the bus protocol had no deployed external
consumers. The change is wire-breaking for any already-serialized CBOR
payload (none existed outside this repo). Every round-trip test passed
without modification — serde handles both encode and decode through the
same Rust types, so the proof that clients still agree with the core
runs as part of `cargo test`.

`contracts/bus-messages.md` now documents the tagging convention as a
first-class principle.

#### Phase 6 — Polish & cross-cutting concerns

- **`core/README.md`** and **`tui/README.md`** (new): per-crate orientation with module maps, usage snippets, and pointers to the spec.
- **`.github/workflows/ci.yml`** (new): GitHub Actions workflow running `cargo fmt --all -- --check`, `cargo clippy --all-targets --workspace -- -D warnings`, `cargo build --workspace`, `cargo test --workspace` — mirrors `scripts/ci.sh` but cached + pinned. Enforces L2 P19 + L2 P10 + L2 Amendment 6.
- **`core/tests/property/provenance_wire.rs`** (new, T068): proptest that every `BusMessage` variant carrying `Provenance` round-trips through both CBOR and JSON with a non-empty `source`. Two generators per pass.
- **`core/tests/cli/version_timing.rs`** (new, T075): benchmark asserting median wall time of `weaver --version` is ≤ 50 ms (SC-006). Runs a warm-up iteration + 5 samples + median-of-5. Prints min/median/max to stderr for diagnostic visibility.
- **`quickstart.md`**: SC-003 example corrected. The original "edit one buffer repeatedly → `facts.length` grows" claim contradicts the data model (facts are keyed by `(entity, attribute)`; re-assertion refreshes provenance but does not add entries). The walkthrough now uses distinct buffer ids to demonstrate array growth, with a note explaining the fact-space semantics.
- **`tasks.md`**: all T064–T070 + T075 marked `[X]`. CHANGELOG `[Unreleased]` promoted to `[0.1.0] — 2026-04-20`.

#### Fix — `weaver --version` build timestamp stuck at 1980

- **Symptom**: `weaver --version` displayed `built: 1980-01-01T00:00:00.000000000Z` in every `cargo build` run from a nix dev shell.
- **Root cause**: `nixpkgs`' stdenv pre-sets `SOURCE_DATE_EPOCH=315532800` (1980-01-01T00:00:00Z) in every `mkShell` environment as a reproducible-build floor for ZIP-format compatibility (nix issue [#20716](https://github.com/NixOS/nixpkgs/issues/20716)). `vergen` honors `SOURCE_DATE_EPOCH` unconditionally, so the misleading placeholder flowed into the binary.
- **Fix (two-pronged)**:
  - `core/build.rs` — removes `SOURCE_DATE_EPOCH` if and only if it equals the exact nix-stdenv sentinel `315532800`. Any intentional value (e.g., a CI release build setting it to the commit timestamp for bit-reproducibility) is preserved. See [reproducible-builds.org](https://reproducible-builds.org/docs/source-date-epoch/) — `315532800` is documented as a ZIP-compat floor via `max(315532800, real_time)`, not a semantic timestamp; clearing it matches the upstream spec's "fall back to system time when unset" expectation.
  - `flake.nix` — `unset SOURCE_DATE_EPOCH` in `shellHook`, eliminating the sentinel at its source for users of the Weaver flake. The `build.rs` filter is the safety net for devenv/mise/direnv/plain shells that might inherit it from elsewhere.
- **L2 tension**: P11 (informative timestamp) vs P19 (reproducible builds). The resolution preserves both: P11 for dev/CI builds (no SOURCE_DATE_EPOCH, real wall time); P19 for release builds (caller sets SOURCE_DATE_EPOCH to the commit timestamp, which is neither unset nor `315532800`, so it's respected).

### Test summary for v0.1.0

- **54 weaver_core unit tests** — domain types, fact space, bus codec + delivery, trace store, behaviors, CLI parsers.
- **13 integration tests** — `core/tests/{behavior,inspect,property,cli}/*.rs` covering US1, US2, US3 scenario + property + timing.
- **2 workspace-level e2e tests** — `tests/e2e/{hello_fact,disconnect}.rs` spawning the `weaver` binary and driving it over the bus.

Total: 69 tests. `scripts/ci.sh` green end-to-end.
