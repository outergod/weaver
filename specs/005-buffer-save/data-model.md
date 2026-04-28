# Data Model — Slice 005 (Buffer Save)

This document captures the new types introduced by slice 005, their wire shapes, validation rules, and the state-transition mapping for accepted/dropped/refused saves. Inherits everything from slice 003's data model (`specs/003-buffer-service/data-model.md`) and slice 004's data model (`specs/004-buffer-edit/data-model.md`) unchanged unless explicitly noted.

## New types

### `EventOutbound`

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventOutbound {
    pub name: String,
    pub target: Option<EntityRef>,
    pub payload: EventPayload,
    pub provenance: Provenance,
}
```

The wire-level shape of an event in flight from a producer to the listener — `Event` minus `id`. The listener allocates a stamped `EventId` on accept (per FR-021 + research §5). Field set mirrors slice-001's canonical `Event` shape (`name` is the wire-stable identifier per L2 P7; `target` carries the entity the event is about, used for filtered subscriptions and `weaver inspect` rendering); `causal_parent` continues to live on `provenance.causal_parent` per slice-001 data-model.md:55, unchanged across slices 002/003/004.

**Wire shape (JSON)**:
```json
{
  "name": "buffer/save",
  "target": 42,
  "payload": {"type": "buffer-save", "payload": {"entity": 42, "version": 7}},
  "provenance": {"source": {"type": "user"}, "timestamp_ns": 1714217040123456789, "causal_parent": null}
}
```

(`Provenance` field names are snake_case on the wire, matching slice-003/004 contracts; `EventPayload` variant tags are kebab-case per Amendment 5 because the enum carries `#[serde(rename_all = "kebab-case")]`.)

**Wire shape (CBOR)**: plain CBOR map carrying the four `EventOutbound` fields as text-string keys (`name`, `target`, `payload`, `provenance`). No CBOR tag.

**Conversion to `Event`**:
```rust
impl Event {
    pub fn from_outbound(id: EventId, outbound: EventOutbound) -> Self {
        Self {
            id,
            name: outbound.name,
            target: outbound.target,
            payload: outbound.payload,
            provenance: outbound.provenance,
        }
    }
}
```

No reverse conversion is provided. Stamped events do not regress to outbound shape.

### `EventPayload::BufferSave`

```rust
pub enum EventPayload {
    BufferOpen { path: String },                                     // existing (slice 003)
    BufferEdit { entity: EntityRef, version: u64, edits: Vec<TextEdit> }, // existing (slice 004)
    BufferSave { entity: EntityRef, version: u64 },                       // NEW (slice 005)
}
```

The bus-level save-dispatch record.

**Fields**:
- `entity` — the buffer to save (canonical entity, derived from `buffer_entity_ref(canonical_path)`).
- `version` — the emitter's snapshot of `buffer/version`. Service accepts iff `version == buffer's current buffer/version`.

Note: no `edits` analogue. Save is a non-mutating operation w.r.t. content (the in-memory `BufferState::content` is the source of truth; save persists those bytes verbatim to disk).

**Delivery class**: lossy (per `EventPayload` convention, consistent with `BufferEdit`).

**Wire shape (JSON, adjacent-tagged kebab-case)**:
```json
{
  "type": "buffer-save",
  "payload": {
    "entity": 42,
    "version": 7
  }
}
```

### `BusMessage<E>` (generic refactor)

```rust
pub enum BusMessage<E> {
    Hello { protocol_version: u8, ... },
    Welcome { actor_identity: ActorIdentity },
    Event(E),                                        // <-- generic over event shape
    FactAssert(Fact),
    FactRetract(FactKey),
    SubscribeFacts(SubscribePattern),
    SubscribeEvents(EventSubscribePattern),
    InspectRequest { request_id: u64, target: InspectTarget },
    InspectResponse { request_id: u64, result: Result<InspectionDetail, InspectionError> },
    EventInspectRequest { request_id: u64, event_id: EventId },
    EventInspectResponse { request_id: u64, result: Result<Event, EventInspectionError> },
    Error { category: String, detail: String },
    Ping,
    Pong,
}

pub type BusMessageInbound  = BusMessage<EventOutbound>;
pub type BusMessageOutbound = BusMessage<Event>;
```

Codec functions are direction-typed:
- `read_message(&mut R) -> Result<BusMessageInbound, CodecError>` — listener-side reception.
- `write_message(&mut W, msg: BusMessageOutbound) -> Result<(), CodecError>` — listener-side broadcast.

Producer-side reception (CLI clients receiving responses): `read_message` returns `BusMessageOutbound`. Producer-side sending: `write_message` takes `BusMessageInbound` (the producer's outbound is the listener's inbound).

Wire byte representation is identical for non-Event variants — `BusMessageInbound::Hello { .. }` and `BusMessageOutbound::Hello { .. }` serialize to the same bytes. Only the Event variant's payload differs.

### `BufferSaveOutcome`

```rust
pub(crate) enum BufferSaveOutcome {
    Saved          { entity: EntityRef, path: PathBuf, version: u64 },
    CleanSaveNoOp  { entity: EntityRef, version: u64 },
    StaleVersion   { event_version: u64, current_version: u64 },
    NotOwned       { entity: EntityRef },
    InodeMismatch  { entity: EntityRef, path: PathBuf, expected: u64, actual: u64 },
    PathMissing    { entity: EntityRef, path: PathBuf },
    TempfileIo     { entity: EntityRef, path: PathBuf, error: io::Error },
    RenameIo       { entity: EntityRef, path: PathBuf, error: io::Error },
}
```

In-process Rust enum returned by `dispatch_buffer_save`. Mirrors slice-004's `BufferEditOutcome` design. Maps 1:1 to diagnostic codes per `research.md §9`. Not a wire type.

### `WriteStep`

```rust
pub(crate) enum WriteStep {
    OpenTempfile,
    WriteContents,
    FsyncTempfile,
    RenameToTarget,
    FsyncParentDir,
}
```

Test-injection hook surface for `atomic_write_with_hooks` (per `research.md §3`). `pub(crate)` for visibility from the `tests/e2e/` integration tests. Not a wire type.

## Inherited types — extended

### `Event` (unchanged shape; production-side semantics revised)

```rust
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub id: EventId,
    pub name: String,
    pub target: Option<EntityRef>,
    pub payload: EventPayload,
    pub provenance: Provenance,
}
```

Field set unchanged from slice 004 — listed here for cross-reference against `EventOutbound` (which is `Event` minus `id`). Under §28(a), `Event` is now produced ONLY by the listener (via `Event::from_outbound`); no longer constructed by producers (per FR-019). Subscribers continue to receive `Event` via `BusMessageOutbound::Event(_)`.

### `EventId` (semantics revised under §28(a))

Wire shape unchanged: 64-bit unsigned integer.

Semantic shift:
- **Before slice 005**: producer-minted from `now_ns()`. `TraceStore::by_event` indexed by producer-minted ID; cross-producer collision was latent (§28).
- **After slice 005**: listener-allocated from `TraceStore::next_event_id` monotonic counter (per `research.md §5`). Producer code never constructs an `EventId` value for outbound events; producers construct `EventOutbound` (no `id`).

`EventId::ZERO` retains its sentinel meaning for "no causal parent" lookups. The `next_event_id` counter starts at `1` to skip ZERO. The slice-004 `lookup_event_for_inspect` ZERO-short-circuit (slice 004 PR #11 commit `f0112d4`) is preserved per FR-024.

### `BufferState` (extended)

Slice 003 declared `BufferState { path, entity, content, memory_digest, last_dirty, last_observable }`. Slice 004 added `apply_edits(&mut self, edits: &[TextEdit]) -> Result<(), ApplyError>`. Slice 005 extends with:

```rust
pub struct BufferState {
    path: PathBuf,
    entity: EntityRef,
    inode: u64,                                 // NEW (slice 005)
    content: Vec<u8>,
    memory_digest: [u8; 32],
    last_dirty: bool,
    last_observable: bool,
}

impl BufferState {
    pub fn save_to_disk(&self, path: &Path) -> SaveOutcome { ... }   // NEW
}
```

The `inode: u64` field is set once at `BufferState::open` time (immediately after path canonicalisation) via `std::fs::metadata(&canonical_path)?.ino()`. Subsequent external mutations do not update this field.

`save_to_disk(&self, path: &Path)` is the in-process save method:
1. Stat the path; if `Err(NotFound)` → `SaveOutcome::PathMissing`.
2. If `metadata.ino() != self.inode` → `SaveOutcome::InodeMismatch { expected: self.inode, actual: metadata.ino() }`.
3. Else, call `atomic_write_with_hooks(path, &self.content, |_| Ok(()))`. Map `Err(io::Error)` to either `TempfileIo` or `RenameIo` based on the failing step.
4. On success: `SaveOutcome::Saved { path: path.to_path_buf() }`.

Note: `save_to_disk` does NOT consult `self.last_dirty`. The clean-save no-op flow is decided by `dispatch_buffer_save` BEFORE calling `save_to_disk`. This keeps `save_to_disk` purely concerned with the disk I/O sequence; the version handshake and dirty-bit branch live in the dispatcher.

### `SaveOutcome` (returned by `BufferState::save_to_disk`)

```rust
pub enum SaveOutcome {
    Saved        { path: PathBuf },
    InodeMismatch { expected: u64, actual: u64 },
    PathMissing,
    TempfileIo   { error: io::Error },
    RenameIo     { error: io::Error },
}
```

A subset of `BufferSaveOutcome` covering only the disk-side outcomes. The dispatcher converts `SaveOutcome` to `BufferSaveOutcome` by adding `entity` + `version` context fields.

## Validation rules (save consumer)

`weaver-buffers`'s `dispatch_buffer_save` performs the following pipeline on each inbound `BufferSave` event. Order is significant; each step that returns an outcome short-circuits.

### R1 — Entity ownership

If `BufferRegistry` does NOT own the `event.entity`:
- Outcome: `BufferSaveOutcome::NotOwned { entity }`.
- Trace: `tracing::debug!` with reason `unowned-entity`.
- No re-emission.

### R2 — Version handshake

If `event.version != current buffer/version`:
- Outcome: `BufferSaveOutcome::StaleVersion { event_version, current_version }`.
- Trace: `tracing::debug!` `WEAVER-SAVE-002` with reason `stale-version`.
- No re-emission.

### R3 — Clean-save branch

If `current buffer/dirty == false` (the buffer is already clean):
- Outcome: `BufferSaveOutcome::CleanSaveNoOp { entity, version }`.
- Trace: `tracing::info!` `WEAVER-SAVE-007` with detail `nothing to save`.
- Re-emission: `FactAssert(buffer/dirty, Bool(false))` with `causal_parent = Some(event.id)`. Idempotent observability for late subscribers.
- **NO disk I/O. NO inode check. NO tempfile.**
- Returns success. Steps R4–R6 do NOT execute.

### R4 — Path/inode identity

`stat(self.path)`:
- If `Err(NotFound)`: outcome `BufferSaveOutcome::PathMissing { entity, path }`. Trace `tracing::warn!` `WEAVER-SAVE-006`. No re-emission.
- If metadata indicates non-regular file (symlink, directory, special): outcome `BufferSaveOutcome::PathMissing { entity, path }` (treated as missing for save purposes). Trace `tracing::warn!` `WEAVER-SAVE-006`.
- If `metadata.ino() != self.inode`: outcome `BufferSaveOutcome::InodeMismatch { entity, path, expected: self.inode, actual: metadata.ino() }`. Trace `tracing::warn!` `WEAVER-SAVE-005` with detail fields `expected-inode`, `actual-inode`.
- Else: continue to R5.

### R5 — Atomic disk write

Call `atomic_write_with_hooks(self.path, &self.content, |_| Ok(()))`:
- On `Err(io::Error)` from steps `OpenTempfile` / `WriteContents` / `FsyncTempfile`: outcome `BufferSaveOutcome::TempfileIo { entity, path, error }`. Trace `tracing::error!` `WEAVER-SAVE-003`. Best-effort tempfile cleanup attempted.
- On `Err(io::Error)` from steps `RenameToTarget` / `FsyncParentDir`: outcome `BufferSaveOutcome::RenameIo { entity, path, error }`. Trace `tracing::error!` `WEAVER-SAVE-004`. Best-effort tempfile cleanup attempted.
- On `Ok(())`: continue to R6.

### R6 — Success re-emission

Outcome: `BufferSaveOutcome::Saved { entity, path, version }`. Trace `tracing::info!` accepted-save event. Re-emission: `FactAssert(buffer/dirty, Bool(false))` with `causal_parent = Some(event.id)` (stamped by listener per FR-021). Authoring identity: `weaver-buffers`'s own `ActorIdentity::Service`.

## State-transition mapping

For an inbound `BufferSave { entity, version }` event with stamped `event.id`:

```
event arrives → R1 ownership → owned ─┬→ R2 version match ─┬→ R3 dirty? ──┬─ false ──→ CleanSaveNoOp + dirty=false re-emit
                                     │                     │              └─ true ───→ R4 inode check ─┬→ ok ───→ R5 atomic write ─┬→ ok ──→ R6 Saved + dirty=false re-emit
                                     │                     │                                          │                          └─ err ──→ TempfileIo or RenameIo (no re-emit)
                                     │                     │                                          ├─ inode delta → InodeMismatch (no re-emit)
                                     │                     │                                          └─ missing ────→ PathMissing (no re-emit)
                                     │                     └─ stale ──→ StaleVersion (no re-emit)
                                     └─ unowned → NotOwned (no re-emit)
```

For each accepted save (`Saved` or `CleanSaveNoOp`):
- pre-state: `buffer/version = N`, `buffer/dirty = D` (where D is `true` for `Saved`, `false` for `CleanSaveNoOp`)
- effects:
  1. (Saved only) on-disk content of `path` is replaced atomically with `BufferState::content` bytes. Parent directory `fsync`ed for durability.
  2. publish `FactAssert(buffer/dirty, Bool(false))` with `causal_parent = Some(event.id)`
- post-state: `buffer/version = N` (unchanged — save is non-mutating w.r.t. version), `buffer/dirty = false`.

`buffer/byte-size`, `buffer/path`, and `buffer/observable` are NOT re-emitted on save (FR-004).

## Invariants

The following invariants hold across slice-005:

- **`save_to_disk` is structurally pure on rejection**: under any failing outcome (`PathMissing`, `InodeMismatch`, `TempfileIo`, `RenameIo`), `BufferState::content` is unchanged; the on-disk file is unchanged (the atomic-rename invariant guarantees this for `RenameIo`; pre-rename failures never opened the target file at all).
- **`save_to_disk` preserves the `memory_digest == sha256(content)` invariant on success**: `BufferState::content` is read-only during save (no mutation); the digest invariant from slice-003/004 is structurally untouched.
- **Disk content matches `BufferState::content` post-success**: under `Saved` outcome, `read(path) == BufferState::content` byte-for-byte (the `rename(2)` atomicity guarantees no partial writes are visible).
- **Inode field is immutable post-open**: `BufferState::inode` is set once at `open` and never updated. External mutations after open do not affect this field; the inode check at save time uses the pristine open-time value.
- **Stamped EventId monotonicity**: every accepted `BufferSave` (and every other accepted event under §28(a)) carries a stamped `EventId` strictly greater than the previous-accepted event's stamped ID, modulo `u64` wraparound (which is unreachable in any plausible single-process trace lifetime).
- **No producer-minted EventId reaches the trace**: the listener rejects any inbound event shape that carries an ID (structurally — there is no inbound shape with an `id` field; the `BusMessageInbound::Event(_)` variant carries `EventOutbound`, which lacks `id`).

## Forward-direction notes

- Slice 006 (agent emitter) will likely add `ActorIdentity::Agent` discrimination at the emitter side; the slice-005 `BufferSave` event variant is identity-agnostic (any `ActorIdentity` is valid).
- Concurrent-mutation guard (in-place external edits without inode change) is FR-026's deferred work; the `save_to_disk` method's content-write path will be the natural place to add a content-digest pre-check at that time.
- Forward-direction queryable rejection observability (`docs/07-open-questions.md §26`) will likely add a `buffer/error/save` (or similar) component to the buffer entity, surfacing the `BufferSaveOutcome::*` taxonomy queryably; slice 005 surfaces these stderr-only.

---

*Phase 1 — data-model.md complete. Wire contracts, CLI surfaces, and quickstart follow.*
