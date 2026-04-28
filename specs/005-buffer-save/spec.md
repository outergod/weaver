# Feature Specification: Buffer Save (Slice 005)

**Feature Branch**: `005-buffer-save`
**Created**: 2026-04-27
**Status**: Draft
**Input**: User description: "Slice 005 — Buffer Save. Disk write-back for buffers opened by `weaver-buffers`, completing the dirty-state ↔ disk-state coupling that slice 004 scaffolded but could not close. Coupled with §28 fold-in: core-assigned globally unique EventIds (option a from `docs/07-open-questions.md §28`). Wire bumps once: `0x04 → 0x05`."

## Clarifications

### Session 2026-04-27

- Q: How does the §28(a) wire shape express "this Event awaits a core-stamped ID"? → A: **ID-stripped envelope.** Producers serialise an `EventOutbound` shape (no `id` field); the listener inflates to `Event { id, .. }` on accept by allocating a stamped EventId from a per-trace monotonic counter. Subscribers always observe `Event` (with `id` populated); they never see `EventOutbound`. No sentinel value (e.g., `EventId::Placeholder`) is introduced — the type system enforces the invariant that outbound events have no ID and at-rest events do. Rationale: sentinel-as-meaning is anti-introspectable (`Everything Is Introspectable`); the placeholder approach would introduce a permanent type-system gap requiring runtime checks at every EventId site forever, whereas ID-stripped envelope pays the migration cost once and the compiler enforces correctness thereafter.
- Q: When `weaver save` targets a clean buffer (`buffer/dirty = false`, no edits since open or last save), what is the consumer's behaviour? → A: **Idempotent re-emission + structured info diagnostic.** The consumer re-emits `buffer/dirty = false` (idempotent observability for late subscribers and replay), AND emits a `WEAVER-SAVE-007 "nothing to save"` structured stderr record at `info` level. The save event is accepted; no disk I/O performed (the inode check is part of the write path and is skipped under clean-save). The trace records BufferSave event + idempotent fact tick + diagnostic, making the save's mode (no-op-by-cleanness) recoverable from observation alone. Info-level matches slice-004's idiom for accepted operations (`watcher/status` transitions, accepted-edit emissions). Pure no-op (without diagnostic) was rejected as anti-introspectable; CLI-side pre-dispatch catch was rejected because it removes the BufferSave event from the trace, losing operator-intent attestation.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Save an edited buffer to disk (Priority: P1)

An operator has an opened buffer, has applied one or more edits via `weaver edit` / `weaver edit-json` (`buffer/dirty = true`), and wants to commit the in-memory content back to disk. The operator runs `weaver save <ENTITY>`, the event dispatches on the bus, `weaver-buffers` validates ownership + version, performs an atomic disk write (tempfile + fsync + rename), and re-emits `buffer/dirty = false`. The operator's command exits immediately on successful dispatch — fire-and-forget, identical UX to `weaver edit`.

**Why this priority**: This is slice 005's defining assertion. It closes the round trip between in-memory editing (slice 004) and on-disk persistence — without save-to-disk the editing surface is a memory simulator. Every dogfooding-ladder slice from here forward (006 agent, 007 tool use, 008 dogfooded loop) presupposes that an LLM-authored or human-authored edit can be persisted. If this story fails, the dirty flag never flips back to clean and the editor cannot stand on its own ontology.

**Independent Test**: An operator starts core + `weaver-buffers ./file.txt` + TUI. After bootstrap, the operator runs `weaver edit ./file.txt 0:0-0:0 "prefix "` (observes `buffer/dirty = true`), then runs `weaver save <entity>`. Within the interactive latency class, the TUI shows `buffer/dirty = false`; `cat ./file.txt` shows the new content with `prefix ` prepended; `buffer/version` is unchanged from its post-edit value (save does not bump version).

**Acceptance Scenarios**:

1. **Given** a buffer at `buffer/version = N`, `buffer/dirty = true` after one or more `weaver edit` invocations, **When** the operator runs `weaver save <ENTITY>`, **Then** the CLI looks up the current `buffer/version` (the canonical source per slice-004 FR-013), dispatches an `EventOutbound` carrying `EventPayload::BufferSave { entity, version: N }`, exits `0`; within the interactive latency class the TUI shows `buffer/dirty = false`; the on-disk file content matches `BufferState::content` byte-for-byte; `buffer/version` stays at `N`.
2. **Given** two concurrent emitters race the same buffer (CLI A runs `weaver save`; CLI B runs `weaver edit` against the same `buffer/version = N`), **When** A's save lands first and B's edit arrives carrying stale `version = N`, **Then** A's save persists the pre-B content; B's edit silently drops at the stale-version gate (slice-004 FR-005); `buffer/version` stays at `N`; `buffer/dirty = false`. The slow emitter gets no error frame; this is consistent with slice-004 last-write-wins.
3. **Given** a save succeeds against a buffer at `version = 7`, **When** the operator runs `weaver inspect --why <entity>:buffer/dirty`, **Then** the walkback resolves to the accepted `BufferSave` event with stamped EventId; the event's provenance renders the CLI emitter's `ActorIdentity::User`.
4. **Given** a save targets an entity that has no `buffer/version` fact (the buffer is not currently owned by any `weaver-buffers` instance), **When** the CLI's pre-dispatch lookup returns no fact, **Then** the CLI exits `1` with a `WEAVER-SAVE-001` "buffer not opened" diagnostic on stderr; no event is dispatched.

---

### User Story 2 — Refuse save when path/inode changed externally (Priority: P2)

An operator opens a buffer, edits it, and intends to save. Between open and save, an external process renames the underlying file (e.g., `git checkout` swapping branches) or deletes it (e.g., `rm`). The save MUST refuse rather than blindly recreate the path with the operator's content — that would silently clobber whatever external state replaced the file. The user gets a structured diagnostic and may choose to re-open the buffer (out-of-scope `save-as` would be the alternative recovery path).

**Why this priority**: This is the load-bearing safety contract of slice 005. The slice-004 commit "no surprising state mutation" extends naturally to disk: a save must not produce a file the operator did not knowingly write. The two failure modes (rename, deletion) are the realistic external-mutation surfaces; locking would not be appropriate at this stage (single-writer process discipline is operator-side, not OS-enforced).

**Independent Test**: Open buffer, edit, externally `mv` the file aside. Run `weaver save`. Assert: WEAVER-SAVE-005 stderr; the moved-aside file is unchanged; nothing exists at the original path. Repeat with `rm` instead of `mv`: assert WEAVER-SAVE-006 stderr; nothing at the original path.

**Acceptance Scenarios**:

1. **Given** a dirty buffer was opened on `./file.txt` (inode `I0`), **When** an external process renames `./file.txt → ./file.txt.bak` between open and save, **Then** the service's pre-rename inode check fails (`./file.txt` either does not exist or has a different inode), the save refuses, `WEAVER-SAVE-005` is emitted on the service's stderr, the in-memory `BufferState` is unchanged, `buffer/dirty` remains `true`, and the original `./file.txt.bak` content is byte-identical to its pre-save state.
2. **Given** a dirty buffer was opened on `./file.txt`, **When** an external process deletes `./file.txt` (`unlink(2)`) between open and save, **Then** the service's pre-rename existence check fails, the save refuses, `WEAVER-SAVE-006` is emitted, no file is created at `./file.txt`, the in-memory `BufferState` is unchanged, `buffer/dirty` remains `true`.
3. **Given** an external process atomically replaces `./file.txt` with new content (a different inode but at the same path — e.g., another editor's own atomic save), **When** the operator runs `weaver save`, **Then** the inode check fires `WEAVER-SAVE-005`; the externally-written content is preserved; the operator's edits remain in-memory and recoverable via re-open + manual reconciliation.

---

### User Story 3 — `weaver inspect --why` resolves source events across concurrent producers (Priority: P3)

A CI test or an operator running multi-process workloads (e.g., a sequence of `weaver edit` and `weaver save` invocations interleaved with `weaver-git-watcher` and `weaver-buffers` poll-tick re-emissions) inspects causal-parent walkbacks across the trace. Under §28(a) (core-assigned EventIds), no walkback ever resolves to a foreign producer's event due to wall-clock-ns collision in `TraceStore::by_event`.

**Why this priority**: This is the semantic gate on §28's fold-in. Slice 004 closed two deterministic collision instances (bootstrap_tick reuse, EventId::ZERO sentinel) but left the cross-producer wall-clock-ns residue. Slice 005's wire bump is the natural amortisation point. If §28(a) ships incorrectly, every future multi-producer slice (006 agent, 007 tool use) inherits the same latent gap and may surface it via dogfooding.

**Independent Test**: Run a stress harness that emits N=1000 events from K=3 producers (CLI edit + CLI save + buffers poll-tick) in tight succession. For each event, capture the stamped EventId from the listener. Walk every fact's `causal_parent` via `weaver inspect --why` and assert every walkback resolves to its actual source producer (verified by cross-checking emitter identity in event provenance against the producer that originated the event).

**Acceptance Scenarios**:

1. **Given** producer A and producer B simultaneously emit `EventOutbound`s at the same wall-clock-ns boundary, **When** the listener stamps each on receipt, **Then** the two events receive distinct stamped EventIds; `TraceStore::by_event` indexes both correctly; `weaver inspect --why` walkbacks resolve to the correct producer in 100% of cases under the stress harness.
2. **Given** a producer attempts to serialise an `Event { id, .. }` (misbehaving / outdated / pre-§28(a) client) onto the bus, **When** the listener's codec attempts to deserialise as `EventOutbound`, **Then** the codec returns a decode error to the producer; no event is appended to the trace; the producer's connection receives the error frame per existing codec-error semantics.

---

### Edge Cases

- **Save against unopened buffer**: pre-dispatch CLI lookup finds no `buffer/version` fact → exit `1` with `WEAVER-SAVE-001`; no event dispatched.
- **Stale version on save**: the save event's `version` ≠ the buffer's current `buffer/version` (a concurrent edit landed between the CLI's lookup and the service's processing) → silent stale-drop (matches slice-004 FR-005 step 2). `buffer/dirty` is NOT re-emitted; the operator's CLI exited `0` with no signal.
- **Disk full / I/O failure on tempfile write**: `WEAVER-SAVE-003` on service stderr; the tempfile is removed (best-effort cleanup); the in-memory `BufferState` is unchanged; `buffer/dirty` stays `true`.
- **Cross-filesystem rename / `EXDEV`**: the tempfile cannot rename because it lives on a different filesystem from the target. Slice 005 places the tempfile in the same directory as the target precisely to avoid this; if it nonetheless fails (e.g., bind-mount edge case), `WEAVER-SAVE-004` fires.
- **Rename(2) `EACCES` / `ENOSPC` / `EROFS`**: `WEAVER-SAVE-004` covers all rename-time I/O failures; the original file is unchanged.
- **Path renamed externally between open and save (inode delta)**: `WEAVER-SAVE-005`; no write performed.
- **Path deleted externally between open and save**: `WEAVER-SAVE-006`; no write performed.
- **Save against a clean buffer (`buffer/dirty = false`)**: clean-save flow per FR-005 — idempotent re-emission of `buffer/dirty = false` + `WEAVER-SAVE-007 "nothing to save"` info-level diagnostic. No disk I/O, no inode check, no tempfile. Trace records the BufferSave event + fact tick + diagnostic, making the no-op-by-cleanness recoverable from observation alone.
- **Concurrent saves on the same buffer**: both saves race the version handshake. The first to land applies (writes disk if dirty; clean-save flow if already clean); the second arrives at the same version (save doesn't bump), passes the version gate, sees `buffer/dirty = false`, and runs the clean-save flow. Trivial convergence.
- **Tempfile naming collision**: the tempfile name MUST include a random suffix to avoid collision with concurrent `weaver save` runs on the same buffer or with operator-created files. On collision, the operation fails with `WEAVER-SAVE-003` and the tempfile is not overwritten.
- **Producer attempts to send `Event` (with `id`) instead of `EventOutbound`**: codec rejects at deserialise; codec error returns to the producer; no event enters the trace.
- **Listener sees an `EventOutbound` whose payload is unknown**: handled per existing decode-error path; payload-decode error precedes ID stamping — i.e., a malformed event never gets a stamped ID and is not indexed.

## Requirements *(mandatory)*

### Functional Requirements

**Save wire surface — new event variant (rides 0x04 → 0x05 bump):**

- **FR-001**: The system MUST extend `EventPayload` with a new variant `BufferSave { entity: EntityRef, version: u64 }`. Wire tag `"buffer-save"` (adjacent-tagged, kebab-case per L2 Amendment 5). The variant MUST serialise via the existing adjacent-tag enum machinery; no new CBOR tags.
- **FR-002**: The bus protocol MUST advance `0x04 → 0x05`. `Hello.protocol_version = 0x05`; mismatched handshakes receive `Error { category: "version-mismatch", detail: "bus protocol 0x05 required; received 0x04" }` and connection close. The bump is MAJOR and amortises both the `BufferSave` variant (FR-001) and the §28(a) wire-shape change (FR-019..FR-024).

**Save consumer flow — `weaver-buffers` extends its dispatcher:**

- **FR-003**: `weaver-buffers` MUST extend its reader-loop to dispatch `BusMessage::Event(Event { payload: EventPayload::BufferSave { entity, version }, .. })` (where `Event` is the post-stamp shape per FR-021) through a handler paralleling the slice-004 `dispatch_buffer_edit` arm. The handler MUST:
  1. Look up the `BufferState` for `entity`; if not owned, silently no-op (event traced at `debug`).
  2. Compare the event's `version` to the buffer's current `buffer/version`. If unequal, silently drop (event traced at `debug` with mismatch detail). No `buffer/dirty` re-emission.
  3. Branch on `buffer/dirty` value. If `false`, run the clean-save flow per FR-005 and return. Otherwise (dirty), proceed with steps 4–6 below.
  4. Confirm path-on-disk identity: `stat(path)` MUST succeed, MUST be a regular file, AND its inode MUST equal the inode captured at `BufferOpen` time. Mismatch fires `WEAVER-SAVE-005`; missing-path fires `WEAVER-SAVE-006`. Both are stderr-only diagnostics (no fact-space surface, matching slice-004 FR-018 idiom). Buffer state and dirty flag are unchanged on refusal.
  5. Atomic disk write: create tempfile in the same directory as `path` with name `.<basename>.weaver-save.<random-suffix>`, write `BufferState::content` bytes, `fsync` the tempfile, `rename(2)` the tempfile to `path`. On any I/O failure during create / write / fsync, emit `WEAVER-SAVE-003`, attempt tempfile cleanup, leave buffer state unchanged. On `rename(2)` failure, emit `WEAVER-SAVE-004`, attempt tempfile cleanup, leave buffer state unchanged.
  6. On success, re-derive `disk_content_digest = sha256(BufferState::content)` (now equal to `memory_digest` because content was just written), and re-emit `buffer/dirty` with the value `memory_digest != disk_content_digest` (which evaluates to `false` post-save). Authoring identity: `weaver-buffers`'s own `ActorIdentity::Service`; `causal_parent = Some(event.id)` where `event.id` is the core-stamped ID per FR-022.
- **FR-004**: The save flow MUST NOT bump `buffer/version`. Save is non-mutating w.r.t. content; the version field describes content version, not save status. `buffer/byte-size` and `buffer/path` and `buffer/observable` are likewise NOT re-emitted.
- **FR-005**: Save against a clean buffer (`buffer/dirty = false` at the time the consumer dispatches) MUST execute the **clean-save flow**: re-emit `buffer/dirty = false` (idempotent observability; same `causal_parent = Some(event.id)` discipline as the dirty-save flow), AND emit a `WEAVER-SAVE-007 "nothing to save: buffer was already clean"` structured stderr diagnostic at `info` level. NO disk I/O is performed. NO inode check (FR-003 step 4) is performed. NO tempfile is created. The clean-save flow is the consumer's complete handling — it does not fall through to the write path.
- **FR-006**: The pre-rename inode check (FR-003 step 4) MUST consult an inode value captured at `BufferOpen` time. Slice 005 extends `BufferState` to record this inode. The capture is one-shot at `BufferOpen`; subsequent external mutations do not update the captured inode (that would defeat the check's purpose).
- **FR-007**: Tempfile cleanup on failure MUST be best-effort (`unlink(2)` of the tempfile path). Cleanup failure is logged at `warn` level but does NOT promote the parent failure's diagnostic class (i.e., a `WEAVER-SAVE-003` whose cleanup also fails remains a `WEAVER-SAVE-003`). The tempfile naming convention (`.<basename>.weaver-save.<random>`) makes orphaned tempfiles identifiable in subsequent operator inspection.

**Save CLI — new `weaver save` subcommand:**

- **FR-008**: The `weaver` CLI MUST grow one new subcommand: `weaver save <ENTITY> [--socket <PATH>]` — positional entity reference (path or `EntityRef`-stringified form, matching the resolver pattern of `weaver inspect`). Fire-and-forget like `weaver edit`. No JSON variant ships in slice 005 (deferred — operator confirmed).
- **FR-009**: `weaver save` MUST source the `BufferSave` event's `version` field by **pre-dispatch lookup** of the current `buffer/version` fact for the target entity (identical mechanism to slice-004 FR-013). If the lookup returns no fact, the CLI exits `1` with a `WEAVER-SAVE-001` "buffer not opened: <ENTITY>" diagnostic on stderr; no event is dispatched.
- **FR-010**: `weaver save` MUST stamp `ActorIdentity::User` on the dispatched event (slice-004 idiom; the agent slice will introduce `ActorIdentity::Agent`).
- **FR-011**: `weaver save` MUST NOT subscribe post-dispatch and MUST NOT block waiting for `buffer/dirty` confirmation. Exit codes:
  - `0` — event dispatched successfully (does NOT imply save applied).
  - `1` — CLI parse error, malformed entity reference, **or pre-dispatch lookup found no `buffer/version` fact (`WEAVER-SAVE-001`)**.
  - `2` — bus unavailable (socket missing, handshake failed); aligns with slice-003/004 convention.
  - **No new exit code for "save refused at the service" or "stale version dropped"** — slice-004's silent-drop posture extends to slice 005; service-side refusals are stderr-only. Operators wanting save confirmation subscribe to `buffer/dirty` post-dispatch.

**WEAVER-SAVE-NNN diagnostics:**

- **FR-012**: `WEAVER-SAVE-001` — buffer not opened (entity unknown / no `buffer/version` fact). Surface: CLI stderr at parse time + exit `1`. Service-side never sees this code (the CLI catches at lookup before dispatch).
- **FR-013**: `WEAVER-SAVE-002` — stale version handshake (event's `version` ≠ current `buffer/version`). Surface: service stderr at `debug` level (matching slice-004 FR-018 idiom — silent drop, no bus-level visibility). The CLI cannot detect this; operators subscribing to `buffer/dirty` see no flip.
- **FR-014**: `WEAVER-SAVE-003` — tempfile create / write / fsync I/O failure. Surface: service stderr at `error` level (this is operator-actionable — disk full, permissions, etc.). Buffer state unchanged; dirty stays `true`. Tempfile cleanup attempted.
- **FR-015**: `WEAVER-SAVE-004` — `rename(2)` I/O failure (`EXDEV`, `ENOSPC`, `EACCES`, `EROFS`, etc.). Surface: service stderr at `error` level. Original disk file is unchanged (atomic-rename invariant). Tempfile cleanup attempted.
- **FR-016**: `WEAVER-SAVE-005` — refusal: path no longer points to the same inode the buffer opened (concurrent external rename or atomic-replace by another editor). Surface: service stderr at `warn` level (operator-recoverable: re-open). No write performed; buffer state unchanged.
- **FR-017**: `WEAVER-SAVE-006` — refusal: path was deleted on disk between open and save. Surface: service stderr at `warn` level. No write performed; buffer state unchanged.
- **FR-017a**: `WEAVER-SAVE-007` — clean-save no-op (`buffer/dirty = false` at the time the consumer dispatches; no disk I/O performed; idempotent `buffer/dirty = false` re-emission still occurs per FR-005). Surface: service stderr at `info` level — operator-informational, matches slice-004's idiom for accepted operations. Buffer state and disk content are unchanged; the diagnostic confirms the save was structurally a no-op due to cleanness rather than a failure.
- **FR-018**: All seven `WEAVER-SAVE-NNN` diagnostics MUST emit structured `tracing` records with fields: `entity`, `path`, `event_id` (the core-stamped ID per FR-022; absent on `WEAVER-SAVE-001`, which fires CLI-side before any event is dispatched), and a code-specific detail field (e.g., `expected_inode` / `actual_inode` for -005, `errno` / `os_error` for -003/-004). Format consistent with slice-004's diagnostic discipline.

**§28(a) fold-in — core-assigned globally unique EventIds:**

- **FR-019**: Producers MUST NOT mint `EventId` from wall-clock-ns (or any other producer-side source). Every existing producer-side mint site MUST migrate to constructing `EventOutbound` (which has no `id` field; see FR-020) instead of `Event`:
  - `core/src/cli/edit.rs` (`weaver edit`, `weaver edit-json`)
  - `buffers/src/publisher.rs` (poll-tick re-emissions, bootstrap_tick)
  - `git-watcher/src/publisher.rs` (poll-tick re-emissions)
  - `core/src/cli/save.rs` (the new `weaver save` per FR-008) — born compliant.
- **FR-020**: The §28(a) wire shape MUST be an **ID-stripped envelope**: producers serialise `EventOutbound { payload, provenance, causal_parent, .. }` (NO `id` field); the listener inflates to `Event { id, payload, provenance, causal_parent, .. }` on accept by allocating a stamped EventId from a per-trace monotonic counter. Subscribers always observe `Event` (with `id` populated); they never see `EventOutbound`. No sentinel value (e.g., `EventId::Placeholder`) is introduced — the type system enforces the invariant that outbound events have no ID and at-rest events do.
- **FR-021**: The core's bus listener MUST be the sole stamping point. On every accepted `BusMessage::Event(event_outbound)`:
  1. `validate_event_envelope` validates the structural shape of `EventOutbound` (provenance well-formedness, payload decode); the absence of `id` is type-system-enforced (no runtime check needed). The listener cannot receive an inbound `Event { id, .. }` shape — the wire type for incoming events is `EventOutbound`.
  2. The listener allocates a fresh stamped `EventId` from a per-trace monotonic counter, constructs `Event { id: stamped, payload, provenance, causal_parent, .. }` from the `EventOutbound`, then proceeds with broadcast and trace insertion.
  3. Subscribers (`weaver-buffers`, `weaver-git-watcher`, future agents) observe `Event` (with stamped IDs) only; they never see `EventOutbound`.
- **FR-022**: All `causal_parent` references MUST use stamped `EventId`s. Re-emitters (`weaver-buffers`, `weaver-git-watcher`) consume events that have already been stamped (per FR-021 step 3); they propagate the stamped ID into `causal_parent` of their fact re-emissions. CLI emitters do not propagate `causal_parent` (they are one-shot, no re-emission).
- **FR-023**: `TraceStore::by_event` MUST be indexed by stamped `EventId`. Under §28(a), no two events share an EventId — last-writer-wins on insert ceases to be a hazard for the user-visible `weaver inspect --why` walkback surface.
- **FR-024**: The slice-004 `EventId::ZERO` consumer-side rejection (closed in slice 004 PR #11 commit `f0112d4` at `lookup_event_for_inspect`) MUST be preserved. ZERO is no longer a producer-side mint hazard under §28(a) (producers do not mint EventIds at all), but the consumer-side guard MUST remain as a defence against pre-fix events sitting in long-running deployment traces (an `EventId::ZERO` may be indexed in `TraceStore::by_event` from before the §28(a) migration; the inspect path must still short-circuit ZERO walkbacks). The producer-side `validate_event_envelope` ZERO-rejection from slice 004 is structurally subsumed by FR-021's type-system-enforced absence of `id` on `EventOutbound`.

**Hygiene (carried from slice-004 session-3 handoff):**

- **FR-025**: `core/src/cli/edit.rs::handle_edit_json` MUST grow a code comment at the post-parse step explicitly documenting why empty `[]` JSON does NOT short-circuit (asymmetric with positional zero-pair). Reference: slice-004 `spec.md §220` reserves wire-level empty `BufferEdit` as a future-tool handshake-probe affordance; the CLI preserves this asymmetry intentionally. Lands as a slice-005 commit per slice-004 drift discipline.

**Known gaps (documented, not closed this slice):**

- **FR-026**: Concurrent-mutation guard (file changed on disk by external editor with same inode/path) is NOT closed. Inode-equality on rename pre-check catches the rename/atomic-replace surface but NOT in-place external edits. Deferred to slice 006 — clusters with agent-emitted edits and §28's revisit trigger #2 (multi-producer `causal_parent` chains).
- **FR-027**: `save-as` for path-less buffers and for "save to a different path" is NOT shipped. Path-less buffers themselves remain out of MVP scope (`docs/07-open-questions.md §21`). Re-pathing an opened buffer is post-MVP UX.
- **FR-028**: Auto-save (timer-driven, dirty-detection-driven) is NOT shipped. Save is operator-explicit only. Auto-save is a UX dimension layered atop the save primitive.
- **FR-029**: Slice-004 FR-019 (unauthenticated edit channel) extends to save: any process with a bus connection can dispatch an `EventOutbound` carrying `EventPayload::BufferSave` with any `ActorIdentity`. The hazard is the same shape as the edit channel and inherits the same deferral. MUST be closed before slice 006 (agent) ships.
- **FR-030**: `TraceStore::by_event` internal hardening beyond §28(a) (e.g., paired with `TraceSequence` lookups for redundancy) is NOT in scope. §28(a) removes the producer-side collision class; any residual internal hardening is a separate slice.
- **FR-031**: Open questions §27 (bounded subscriber queues) and §29 (frame headroom asymmetry) are NOT closed. Their revisit triggers fire independently of slice 005's wire bump.

### Key Entities

- **`BufferSave` event payload**: The bus-level save-dispatch record. Fields: `entity: EntityRef` (the buffer to save), `version: u64` (emitter's snapshot of `buffer/version`; the handshake is `emitted_version == current_version`). Wire identity: an adjacent-tagged enum variant with tag `"buffer-save"`. Delivery class: lossy (per `EventPayload` convention; consistent with `BufferEdit`). Carried inside `EventOutbound` outbound and inside `Event` at-rest / on broadcast.
- **`EventOutbound`**: the wire-level shape of an event in flight from a producer to the listener. Fields: `payload, provenance, causal_parent, ..` — no `id`. Producers serialise this; the listener deserialises this, allocates a stamped EventId, and constructs `Event { id, payload, provenance, causal_parent, .. }` for trace insertion and subscriber broadcast. The type-system distinction between `EventOutbound` and `Event` enforces the invariant that outbound events carry no ID and at-rest events do.
- **`buffer/dirty` fact family** (unchanged shape from slice 004; re-emitted on accepted save): `FactValue::Bool`. Slice 004 always re-emitted `true` post-edit because no save path existed; slice 005 makes it flip to `false` post-save (and re-emit `false` idempotently under the clean-save flow). Schema unchanged.
- **`BufferState` inode capture**: at `BufferOpen` time, the service captures the file's inode and stores it on `BufferState`. Used at save time to detect concurrent external rename / atomic-replace. The captured inode is immutable for the lifetime of the buffer.
- **`WEAVER-SAVE-NNN` diagnostic taxonomy**: seven structured stderr codes (-001 through -007). Format mirrors slice-004's `WEAVER-EDIT-NNN` discipline: tracing fields, code-specific detail, no fact-space surface. Forward-direction queryable error component (`docs/07-open-questions.md §26`) inherits.
- **Stamped `EventId` (§28(a))**: a 64-bit monotonic counter allocated by the core's bus listener on event accept. Replaces the previous producer-minted wall-clock-ns scheme. Guarantees: (1) globally unique per trace; (2) monotonically increasing in stamp order; (3) NEVER `EventId::ZERO` (slice-004 ZERO-rejection rationale extends — pre-§28(a) traces may contain ZERO entries that the consumer-side guard short-circuits).

## Affected Public Surfaces *(mandatory)*

### Fact Families & Authorities

- **Authority**: `weaver-buffers` retains single-writer authority over the `buffer/*` family for every buffer entity it owns. Slice 005 extends the existing service's mutation surface (now writes to disk on demand) without introducing a new authority.
- **Fact families touched**:
  - `buffer/dirty` — **modified** (re-emitted on accepted save; value `false` post-write or post-clean-save no-op; shape and authority unchanged).
  - `buffer/version` — **read-only on save** (save is non-mutating w.r.t. version; bumping was a slice-004 edit-only effect).
  - `buffer/path`, `buffer/byte-size`, `buffer/observable`, `watcher/status` — **read-only** (save does not affect these).
- **Schema impact**: **Additive** at every fact-family level; no `buffer/*` family's value type, cardinality, or authority changes. The wire-level break is at the `EventPayload` and event-envelope layers, NOT at the fact layer.

### Other Public Surfaces

- **Bus protocol**: bumps from `0x04` to `0x05`. Enumerated changes:
  - `EventPayload::BufferSave { entity: EntityRef, version: u64 }` added.
  - `EventOutbound` struct added as the wire-level outbound shape (no `id` field); `Event` (with `id`) remains the at-rest / broadcast / trace shape. Producers' `BusMessage::Event(_)` payload type changes from `Event` to `EventOutbound`; subscribers continue to receive `Event` per FR-021.
  - `Hello.protocol_version` advances `0x04 → 0x05`; mismatched clients receive `Error { category: "version-mismatch", .. }` and close.
- **CBOR tag scheme**: no new CBOR tags. `EventOutbound` rides plain struct serialisation (consistent with how `Event` itself rides plain serialisation in slice 004). The `EventId` wire shape is unchanged for at-rest occurrences (`EventOutbound` simply omits the field).
- **Action-type identifiers**: not affected.
- **CLI flags + structured output shape**:
  - Existing `weaver` CLI: `save` subcommand **added** (MINOR additive). `weaver --version` JSON field `bus_protocol` advances `0.4.0 → 0.5.0`.
  - Existing `weaver-buffers`, `weaver-git-watcher`, `weaver-tui`: `--version` JSON field `bus_protocol` advances `0.4.0 → 0.5.0` (constant-driven).
  - `weaver inspect`: shape unchanged; `--why` walks now traverse `BufferSave` events and stamped `EventId`s transparently.
- **Configuration schema**: no changes.
- **Steel host primitive ABI**: not affected.

### Failure Modes *(mandatory)*

- **Degradation taxonomy**: unchanged from slice 004. `watcher/status` transitions are not driven by save activity. A catastrophic save invariant violation (e.g., post-rename `stat` shows the new file's size mismatches the bytes written) is treated as an unrecoverable programmer bug — `panic!` / process exit / core's `release_connection` retracts every `buffer/*` fact.
- **Failure facts**: no new fact families introduced for save-failure surfacing. All save failures are stderr-only diagnostics per FR-012..FR-018, consistent with slice-004 FR-018's posture. Forward direction (queryable error component) is `docs/07-open-questions.md §26`.
- **Emitter-side failure**: `weaver save` exit codes per FR-011.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-501**: Following `weaver edit <PATH> <RANGE> <TEXT>` then `weaver save <ENTITY>` against an opened buffer, an observer subscribed to `buffer/*` observes `buffer/dirty = false` within the interactive latency class (≤500 ms median, matching slice-004 SC-401 budget). The on-disk file content equals `BufferState::content` byte-for-byte.
- **SC-502**: Under externally-induced rename (`mv ./file ./file.bak` between open and save), `weaver save` produces a `WEAVER-SAVE-005` stderr record; the on-disk content of `./file.bak` is byte-identical to its pre-save state; nothing exists at `./file`; the in-memory buffer state is unchanged; `buffer/dirty` remains `true`.
- **SC-503**: Under externally-induced deletion (`rm ./file` between open and save), `weaver save` produces a `WEAVER-SAVE-006` stderr record; nothing is created at `./file`; the in-memory buffer state is unchanged; `buffer/dirty` remains `true`.
- **SC-504**: Under simulated I/O failure between tempfile write and `rename(2)` (e.g., test harness intercepts the rename syscall and forces it to fail with `ENOSPC`), the original disk file is byte-identical to its pre-save state — the atomic-rename invariant holds. `WEAVER-SAVE-004` is emitted.
- **SC-505**: A multi-producer stress harness (≥1000 events from ≥3 distinct producers in tight succession) produces `weaver inspect --why` walkbacks that resolve to the correct source producer in 100% of cases. Validates §28(a) — no last-writer-wins collision in `TraceStore::by_event` for any pair of distinct events.
- **SC-506**: A producer that attempts to send a wire-level `Event` (with `id`) instead of `EventOutbound` receives a codec decode error from the listener; the event is not appended to the trace; the producer's connection is closed per existing codec-error semantics.
- **SC-507**: `weaver save <ENTITY>` against a clean buffer (`buffer/dirty = false`) produces a `WEAVER-SAVE-007` info-level stderr record; an idempotent `buffer/dirty = false` fact is re-emitted with `causal_parent` pointing at the new BufferSave event; no disk I/O occurs (verified by mtime preservation on the underlying file).

## Known Hazards

*Slice-005 LIMITATIONS, not requirements. Documented because dependent slices need to know what slice 005 does NOT solve.*

- **Unauthenticated edit/save channel** (extends slice-004 FR-019): any process with a bus connection can dispatch an `EventOutbound` carrying `EventPayload::BufferSave` with any `ActorIdentity`. The hazard shape is identical to slice 004's. Closing requires a wire-level handshake binding `ActorIdentity` to the connection at handshake time. Deferred to a future soundness slice; MUST be closed before slice 006 (agent) ships.
- **In-place external edits** (without inode change): an external process that modifies the file's content in place (e.g., `>>` append, `truncate(2)` to zero, in-place `sed -i` on filesystems that preserve inode) is NOT detected by the inode-equality check. The save will overwrite the externally-modified content with the buffer's in-memory content. Closing this is slice-006 territory (concurrent-mutation guard).
- **No save-confirmation signal on the wire**: lossy event delivery means `weaver save` cannot distinguish "applied" from "dropped on the wire" from "stale-version rejected" from "I/O failed at service" from "refused on inode change" from "no-op on clean buffer." Slice 005 inherits slice 004's accepted UX cost. Operators needing confirmation subscribe to `buffer/dirty` post-dispatch.
- **No queryable rejection observability**: `WEAVER-SAVE-NNN` codes are stderr-only. External consumers (the future agent on the bus) cannot observe their own save rejections without polling `buffer/dirty`. Forward direction is the same as slice-004 FR-018 (queryable error component, `docs/07-open-questions.md §26`).
- **Tempfile orphans**: I/O failure that prevents tempfile cleanup leaves dot-prefixed `.<basename>.weaver-save.<random>` files on disk. The naming convention makes them identifiable for operator gc, but no automatic cleanup runs. Operator-actionable; not a service-side hazard.
- **No content-version disk metadata**: save writes raw content; it does not stamp `buffer/version` or save-time metadata into a sidecar file. An external observer of the saved file cannot tell which Weaver version produced it. Not in scope; out-of-band metadata is a future concern if dogfooding demands it.
- **§28(a) trace-store internal hardening**: the user-visible collision surface closes; internal `by_event` map is structurally sound under §28(a) (stamped IDs are unique by construction). No further hardening planned this slice.
- **§27 (subscriber bounding) and §29 (frame headroom asymmetry)**: not closed. Their revisit triggers fire independently of slice 005's wire bump.

## Assumptions

*Commitments made when the feature description did not specify certain details; revisited in `/speckit.clarify` if any prove load-bearing.*

- **Save is fire-and-forget at the CLI layer**: identical UX to `weaver edit`. No exit-code distinction between dispatched-and-applied vs dispatched-and-dropped. Locked from operator confirmation; NOT revisited in clarify.
- **Save event delivery class is lossy**: consistent with `BufferEdit`. Save's correctness contract is structural (atomic rename or no change; failure produces stderr diagnostic); delivery-class authoritative would require a structured response wire shape that slice 005 does NOT introduce. Operators wanting confirmation subscribe to `buffer/dirty`. NOT a clarification.
- **Atomic-rename idiom**: tempfile-in-same-dir + write + fsync(tempfile) + rename(2) is the canonical POSIX idiom and the only correct path. fsync of the parent directory after rename is NOT required for save correctness (the rename itself is atomic on POSIX); whether to add it for crash-durability post-rename is an implementation detail (see plan).
- **Tempfile naming**: `.<basename>.weaver-save.<random-suffix>` co-located in the target directory. Dot-prefix to skip from common globs; `weaver-save` infix to identify origin during operator inspection of orphaned tempfiles; `<random-suffix>` to avoid collision under concurrent saves.
- **Inode capture at BufferOpen**: extends `BufferState` with a captured inode field. Slice-003 canonicalisation already resolves the path; the inode is read at the same time as content via `fstat(2)` after `open(2)`. No race between path canonicalisation and inode capture.
- **Save against a clean buffer is a consumer-side no-op success with structured info diagnostic** (Q2 resolution): clean-save flow per FR-005 — re-emit `buffer/dirty = false` (idempotent observability for late subscribers and replay), AND emit `WEAVER-SAVE-007 "nothing to save"` at info level. NO disk I/O, NO inode check. The trace records the BufferSave event + idempotent fact tick + structured diagnostic, making the save's mode (no-op-by-cleanness) recoverable from observation alone. Aligns with `Everything Is Introspectable` — pure no-op (without the diagnostic) would render the save's mode externally unobservable; the diagnostic IS the introspection contract. Mirrors slice-004 FR-008 (empty-batch edit is a valid no-op) extended with explicit informational surfacing.
- **§28(a) wire shape: ID-stripped envelope** (Q1 resolution): producers serialise `EventOutbound` (no `id`); listener inflates to `Event { id, .. }` on accept. Type-system enforces the invariant that outbound events carry no ID and at-rest events do. Sentinel-based alternatives (`EventId::Placeholder`) were rejected as anti-introspectable: a sentinel value is meaningful only at one wire-passage moment, requiring runtime checks at every EventId site forever vs. one-time migration cost of two struct types. Per-producer-counter alternative (option C) was rejected as scope-explosive (producer-id binding + counter persistence + replay semantics).
- **Listener as sole stamping point**: under §28(a), the bus listener is the unique stamping authority. The dispatcher (`process_event`) sees stamped events only. This preserves slice-004's separation between transport-layer envelope validation (listener) and event-routing (dispatcher).
- **`weaver save` accepts entity references in the same form as `weaver inspect`**: positional argument resolves via the existing entity-resolver (path → `EntityRef` via canonicalisation; `EntityRef`-stringified form passes through). No new resolver shape is introduced.
- **Service-side stderr verbosity**: `WEAVER-SAVE-005`, `-006` (refusals — operator-recoverable) emit at `warn` level. `WEAVER-SAVE-003`, `-004` (I/O errors — operator-actionable) emit at `error` level. `WEAVER-SAVE-007` (clean-save no-op — operator-informational) emits at `info` level. `WEAVER-SAVE-002` (stale-drop — silent under slice-004 idiom) emits at `debug` level. `WEAVER-SAVE-001` (CLI-side parse-time check) emits at `error` level on the CLI's stderr.
- **Concurrent-save discipline**: two `weaver save` invocations on the same buffer race the version handshake. The first to land applies (writes disk if dirty; clean-save flow if already clean). The second arrives at the same version (since save doesn't bump version) — passes the version handshake, sees `buffer/dirty = false` (set by the first save's flip), and runs the clean-save flow per FR-005: emits `WEAVER-SAVE-007` + idempotent re-emission. No new failure mode; concurrent saves on the same buffer trivially converge.

## Dependencies

- **Slice 004 (Buffer Edit)** — slice 005 consumes the `BufferState` mutation API, the dispatcher seam, and the `buffer/dirty` re-emission mechanism. Inode capture extends `BufferState` (slice 005 modifies a field that slice 003 introduced).
- **Slice 003 (Buffer Service)** — `BufferRegistry`, path canonicalisation, `BufferOpen` flow are all consumed verbatim. The new inode-capture happens at the same point as content read.
- **Slice 002 (Git-Watcher)** — `ActorIdentity::User` reuse; the `validate_event_envelope` listener-side guard (slice-004 origin) is restructured under §28(a) — its ID-rejection responsibility is subsumed by the type-system distinction between `EventOutbound` and `Event`.
- **Slice 001 (Hello-fact)** — no direct dependency.
- **L1 Constitution §3 (Fact space + events)**: events commute; save is just another event.
- **L1 Constitution §17 (Multi-actor coherence)**: `causal_parent` chains under §28(a) preserve the identity-attribution semantics across multiple producers.
- **L2 Amendment 1 (Conventional Commits)**: BREAKING footer on the protocol-bump commit.
- **L2 Amendment 5 (Wire-stability)**: kebab-case for the new variant tag and the `EventOutbound` struct's serialised field names.
- **`docs/07-open-questions.md §28`**: option (a) chosen and resolved by this slice. The §28 entry is updated to RESOLVED status as part of this slice. The wire-shape resolution is the ID-stripped envelope variant of (a), per Q1 resolution above.
