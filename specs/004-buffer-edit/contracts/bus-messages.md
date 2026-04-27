# Bus Message Contracts — Slice 004

CBOR-encoded messages on the local Unix-domain-socket bus between `weaver` (core), `weaver-tui`, `weaver-git-watcher`, `weaver-buffers`, and any future client. Per L2 P5 / P7 / P8 and `docs/02-architecture.md §3.1`.

**This slice introduces a breaking wire change.** Bus protocol advances **0x03 → 0x04**. Changes:

- `EventPayload::BufferEdit { entity: EntityRef, version: u64, edits: Vec<TextEdit> }` is ADDED.
- Supporting struct types `TextEdit { range: Range, new_text: String }`, `Range { start: Position, end: Position }`, `Position { line: u32, character: u32 }` are ADDED. No new CBOR tags are introduced — the new types ride plain ciborium struct serialisation through the existing adjacent-tag enum machinery.
- `EventSubscribePattern { PayloadType(String) }` enum and `BusMessage::SubscribeEvents(EventSubscribePattern)` variant are ADDED — the bus gains lossy-class **event broadcast** to external subscribers, parallel to the existing fact subscription. See `research.md §13` for the gap-and-resolution context.
- `InspectionDetail` gains a required `value: FactValue` field. Slices 001-003 only carried provenance (where the fact came from); slice 004's `weaver edit` emitter needs the fact value (the current `buffer/version`) to construct the `BufferEdit` envelope. Required-field on the wire — the 0x04 protocol-mismatch handshake rejects mixed-version clients, so no backward-compat shim is needed. See `research.md §2`.
- `BusMessage::EventInspectRequest { request_id, event_id }` and `EventInspectResponse { request_id, result: Result<Event, EventInspectionError> }` are ADDED — event-by-id lookup that powers `weaver inspect --why`'s chain walk from a fact to its source-event's `ActorIdentity` (FR-016 + SC-405). See `research.md §14`.

Old (0x03) clients cannot connect; the handshake rejects mismatched versions with a structured error. No provenance-shape change; `ActorIdentity` (CBOR tag 1002) is unchanged from slice 002. The `ActorIdentity::User` variant — reserved at slice 002 — has its first production use this slice as the emitter identity stamped by `weaver edit` / `weaver edit-json`.

## Naming conventions

Unchanged from slices 002/003 (Amendment 5). Identifier values on the wire are **kebab-case**; struct field names are `snake_case` in Rust / `camelCase` in JavaScript (or kebab-case via `#[serde(rename_all = "kebab-case")]` on JSON; `new_text` Rust → `new-text` JSON). Behavior identifiers use `/` as namespace separator; fact attributes are kebab-case with `/`-delimited namespaces (`buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`, `buffer/version`).

## Wire tagging convention

Unchanged from slice 003. Adjacent-tagged sum types (`"type"` discriminator + content field).

**Sum-type table (slice 004 delta)**:

| Enum             | Content field | Example (JSON)                                                              |
|------------------|---------------|-----------------------------------------------------------------------------|
| `BusMessage`     | `payload`     | `{"type":"event","payload":{...}}` / `{"type":"subscribe-events","payload":{...}}` (NEW) |
| `ActorIdentity`  | `id` or variant-specific fields | `{"type":"user"}` (slice 004 — first production use of `User` variant) |
| `SubscribePattern`| `pattern`    | `{"type":"family-prefix","pattern":"buffer/"}` (facts; unchanged)           |
| `EventSubscribePattern` | `pattern`     | `{"type":"payload-type","pattern":"buffer-edit"}` (NEW; symmetric with `SubscribePattern`'s content field) |
| `FactValue`      | `value`       | `{"type":"u64","value":12345}` (slice 003) / `{"type":"bool","value":true}` |
| `EventPayload`   | `payload` (struct variants only) | `{"type":"buffer-edit","payload":{"entity":42,"version":7,"edits":[...]}}` (NEW) |
| `LifecycleSignal`| —             | Unit-only; serializes as bare string: `"ready"`, `"degraded"`, etc.         |

Plain structs (`TextEdit`, `Range`, `Position`) serialise as CBOR maps / JSON objects with no enum tagging — they are not sum types.

## Wire framing

Unchanged from slices 001/002/003. 4-byte big-endian length prefix; one frame per message; **64 KiB max** (`weaver_core::bus::codec::MAX_FRAME_SIZE`).

The wire-frame limit is the **sole bound on `Vec<TextEdit>::len()`** in slice 004 (per spec Clarifications Q3). `weaver edit-json` MUST pre-check the serialised `BufferEdit` event size at CLI parse time (per CLI surface FR-015) and reject oversized inputs with `WEAVER-EDIT-004` exit 1. The pre-check uses `MAX_EVENT_INGEST_FRAME` (= `MAX_FRAME_SIZE` − `RESPONSE_WRAPPER_HEADROOM` = 65 280 bytes), reserving 256 bytes of headroom so the same `Event`, when wrapped in `BusMessage::EventInspectResponse` for `weaver inspect --why` walkback, still fits within the wire frame.

## Weaver CBOR tag registry (slice 004)

| Tag number | Meaning | Encoded representation | Status |
|---|---|---|---|
| 1000 | `EntityRef` | CBOR unsigned int | existing (slice 001) |
| 1001 | `Keyword` (slash-namespaced symbol) | CBOR text string | existing (slice 001) |
| 1002 | `ActorIdentity` | CBOR map (adjacent-tagged) | existing (slice 002); **unchanged** |

**No new CBOR tags added this slice.** Precedent: slice 003's `FactValue::U64` and `EventPayload::BufferOpen` shipped as new variants/types without new tags, riding plain ciborium serialisation. Slice 004's `EventPayload::BufferEdit`, `TextEdit`, `Range`, `Position` follow the same pattern.

## Connection lifecycle

Unchanged shape; the handshake carries the new protocol version.

1. **Connect**: client opens a stream to the core's socket.
2. **Handshake**: client sends `Hello { protocol_version: 0x04, client_kind: "..." }`.
   - **0x04**: core responds with `Lifecycle(Started)` then `Lifecycle(Ready)`.
   - **0x03** or lower (any mismatch): core responds `Error { category: "version-mismatch", detail: "bus protocol 0x04 required; received 0x03" }` and closes.
3. **Subscribe / interact**: fact subscription via `BusMessage::Subscribe(SubscribePattern)` as in slices 001/002/003 (`family-prefix` patterns; `buffer/` covers the re-emitted `buffer/byte-size`/`buffer/version`/`buffer/dirty` facts on accepted edits). **NEW** event subscription via `BusMessage::SubscribeEvents(EventSubscribePattern)`: clients receive `BusMessage::Event(event)` frames whose `EventPayload` discriminant matches the pattern's payload-type string. Lossy-class delivery (no replay, no per-publisher sequence). `weaver-buffers` subscribes to `payload-type=buffer-edit` post-handshake to receive edit dispatches from `weaver edit` / `weaver edit-json`.
4. **Disconnect**: either side may close the stream; traces record the disconnection. No slice-004 changes to the disconnect path.

## Provenance shape (unchanged from slice 002)

`Provenance` is the per-message attribution record carried on every `Event`, `FactAssert`, `FactRetract`, and every trace entry.

```text
Provenance {
    source: ActorIdentity,              // CBOR tag 1002 — unchanged from slice 002
    timestamp_ns: u64,                  // monotonic nanoseconds since process start
    causal_parent: Option<EventId>      // event that caused this, if any
}
```

**`ActorIdentity::User` first production use**:

```json
{
  "source": {"type": "user"},
  "timestamp_ns": 1_234_567_890,
  "causal_parent": null
}
```

`weaver edit` and `weaver edit-json` stamp `ActorIdentity::User` on emitted `BufferEdit` events. The `User` variant was reserved at slice 002 for human-initiated CLI actions; slice 004 wires it for the first time. The variant carries no fields (unit variant); the timestamp distinguishes successive User-emitted events.

## Fact families introduced (delta from slice 003)

**No new fact families this slice.**

Existing families' authority is unchanged; mutation consumers change as follows:

| Family | Authority | Change in slice 004 |
|---|---|---|
| `buffer/path` | `weaver-buffers` | unchanged — bootstrap-only; never re-emitted by edit path |
| `buffer/byte-size` | `weaver-buffers` | re-emitted on accepted `BufferEdit` with new in-memory byte length; `causal_parent = Some(event.id)` |
| `buffer/dirty` | `weaver-buffers` | re-emitted on accepted `BufferEdit` with `memory_digest != sha256(disk_content)` (in slice 004 always `true` post-edit) |
| `buffer/observable` | `weaver-buffers` | unchanged — never re-emitted by edit path |
| `buffer/version` | `weaver-buffers` | gains a mutation consumer — bumps by exactly 1 per accepted `BufferEdit`; bootstrap shape unchanged from slice-003 PR #10 |
| `watcher/status` | `weaver-buffers` (and slice-002 git-watcher) | unchanged — edits do not flip lifecycle |

## Message-by-message contract (delta from slice 003)

### `Hello` — CHANGED

```text
Hello {
    protocol_version: u8,               // 0x04 in this slice (was 0x03)
    client_kind: String
}
```

Version mismatch triggers `Error { category: "version-mismatch", detail: "bus protocol 0x04 required; received 0x03" }` followed by close.

### `EventPayload` — CHANGED (one variant added)

```text
EventPayload = enum {
    BufferOpen { path: String },                                         // existing (slice 003)
    BufferEdit {                                                          // NEW — slice 004
        entity: EntityRef,
        version: u64,
        edits: Vec<TextEdit>,
    },
}
```

**Wire shape (adjacent-tagged, kebab-case)**:

```json
{
  "type": "buffer-edit",
  "payload": {
    "entity": 42,
    "version": 7,
    "edits": [
      {
        "range": {
          "start": {"line": 0, "character": 0},
          "end": {"line": 0, "character": 0}
        },
        "new-text": "hello "
      }
    ]
  }
}
```

**Semantics**:

- **Version handshake**: service accepts iff `event.version == buffer's current buffer/version`. Mismatched (stale or future) events are silently dropped (no wire response), traced at `tracing::debug` level only.
- **Atomic batch**: every `TextEdit` is validated against current in-memory content before any edit applies. Validation failure on any edit drops the entire batch; no partial application is observable.
- **Apply order**: descending-offset (LSP-compatible). Applies later positions first to avoid invalidating earlier positions.
- **Empty `edits: []`**: well-formed no-op; service traces at `debug`, no `buffer/version` bump.
- **Idempotence under repeat-delivery**: no explicit event-id deduplication; the version-handshake mechanically de-duplicates same-event re-delivery (the second receipt's `version` is stale post-bump and silently drops).

### Supporting struct types — NEW (slice 004)

#### `Position`

```text
Position {
    line: u32,
    character: u32        // UTF-8 byte offset within the line's content
}
```

Wire shape (JSON): `{"line": 42, "character": 12}`. Plain CBOR map (no tag).

`character` counts **UTF-8 bytes within the line's content**, NOT UTF-16 code units. The encoding choice is deliberate (see `specs/004-buffer-edit/spec.md §Assumptions` for rationale; LSP 3.17 `positionEncodings` negotiation makes UTF-8 first-class for future LSP interop).

Endpoints landing mid-codepoint (e.g., inside a multi-byte UTF-8 sequence) are rejected at validation; consumers MUST use Rust's `str::is_char_boundary` or equivalent on the line's bytes.

#### `Range`

```text
Range {
    start: Position,
    end: Position         // exclusive
}
```

Half-open interval. `start <= end` (lexicographic compare on `(line, character)`); strict `start > end` is a validation failure.

Wire shape (JSON): `{"start": {"line":0, "character":0}, "end": {"line":0, "character":5}}`.

#### `TextEdit`

```text
TextEdit {
    range: Range,
    new_text: String      // UTF-8; field renamed to `new-text` on the wire
}
```

Replace the bytes within `range` by `new_text.as_bytes()`.

- **Pure-insert**: `range.start == range.end AND new_text != ""`.
- **Pure-delete**: `range.start < range.end AND new_text == ""`.
- **Nothing-edit (rejected)**: `range.start == range.end AND new_text == ""`.

Wire shape (JSON): `{"range": {...}, "new-text": "replacement"}`.

### `FactAssert` / `FactRetract` — unchanged shape; new causal-parent convention for edit re-emit

Delivery class remains authoritative. `causal_parent` usage convention added for slice 004:

- **Per-edit fact re-emission**: `buffer/byte-size`, `buffer/version`, and `buffer/dirty` re-emitted on an accepted `BufferEdit` share `causal_parent = Some(event.id)`. `weaver inspect --why <entity>:buffer/version` walks from the version fact to the `BufferEdit` event.

### Other messages — unchanged shape

`Event` (envelope), `Subscribe`, `SubscribeAck`, `InspectRequest`, `InspectResponse`, `StatusRequest`, `StatusResponse`, `Error` — no shape changes. The inspection render surface accommodates `EventPayload::BufferEdit` events transparently via existing causal-parent machinery; no new fields introduced.

### `EventInspectRequest` / `EventInspectResponse` — NEW (slice 004)

```text
BusMessage::EventInspectRequest { request_id: u64, event_id: EventId }
BusMessage::EventInspectResponse {
    request_id: u64,
    result: Result<Event, EventInspectionError>,
}

EventInspectionError = enum {
    EventNotFound,
}
```

Event-by-id lookup against the core's trace. Mirrors `InspectRequest` / `InspectResponse`'s shape (request_id correlation; sum-type result). The full [`Event`] envelope is returned on success — the trace already holds it, and copying once for the response is cheaper than designing a sub-shape. `EventInspectionError::EventNotFound` is the only failure variant this slice ships; `Hello`-pre-handshake and other protocol violations are rejected at the listener boundary, not as `EventInspectResponse` errors.

`weaver inspect --why <fact-key>` chains: send `InspectRequest(fact-key)`; take `source_event` from the returned `InspectionDetail`; send `EventInspectRequest { event_id: source_event }`; render the walkback JSON whose `event.provenance` carries the original emitter's `ActorIdentity`. See `research.md §14` and `cli-surfaces.md §weaver inspect --why`.

### `SubscribeEvents` — NEW (slice 004)

```text
BusMessage::SubscribeEvents(EventSubscribePattern)
```

Lossy-class event subscription. The core registers the connection's pattern and forwards every `BusMessage::Event(event)` whose `EventPayload` adjacent-tag matches the pattern. Replied with the existing `BusMessage::SubscribeAck { sequence: 0 }` (events have no per-publisher sequence; the ack confirms the subscription is established).

A connection MAY hold both a fact subscription and an event subscription concurrently; the listener's `select!` arms drain whichever channel produces work. A second `SubscribeEvents` on the same connection replaces the prior pattern (last-wins; consistent with the fact-subscription convention).

**Wire shape (JSON, adjacent-tagged kebab-case)**:

```json
{
  "type": "subscribe-events",
  "payload": {"type": "payload-type", "pattern": "buffer-edit"}
}
```

#### `EventSubscribePattern`

```text
EventSubscribePattern = enum {
    PayloadType(String),                                                // NEW — slice 004
}
```

Matches `Event` whose `payload.type_tag()` equals the pattern's `String` (e.g., `"buffer-edit"`, `"buffer-open"`). Target-entity filtering is the subscriber's responsibility (e.g., `weaver-buffers`'s `dispatch_buffer_edit` returns `NotOwned` for entities outside its registry).

**Future variants** (out of slice-004 scope; documented for the design ceiling): `TargetEntity(EntityRef)` for per-entity filtering; `EventNamePrefix(String)` for `event.name`-based filtering. Both are deferred until a concrete user case appears.

### Event delivery semantics (slice 004)

When the core's `dispatcher.process_event(event)` accepts an event:

1. The event is appended to the trace (existing slice-001 behavior).
2. Registered in-process behaviors fire (existing slice-001 behavior).
3. **NEW**: every active event subscription whose `EventSubscribePattern` matches the event's payload type receives a clone via its mpsc channel; the per-connection listener task forwards it as `BusMessage::Event(event)`.

The subscription registry is held on `Dispatcher` (`event_subscriptions`); subscriber lifecycle is implicit via `mpsc` send-failure (when the subscriber's connection closes, the next broadcast attempt fails and the subscription is dropped on the next pass).

Lossy-class delivery in slice 004 is implemented with `tokio::sync::mpsc::unbounded_channel` for parity with the existing fact-broadcast (which is also unbounded in slices 001–003 despite being authoritative-class). Bounded queues with drop-oldest semantics are deferred to the same future infrastructure slice that bounds fact subscriptions.

## Failure modes (P16 alignment)

| Failure | Wire behavior | Trace consequence |
|---|---|---|
| Client sends `Hello.protocol_version = 0x03` (or lower) | Core sends `Error { category: "version-mismatch", ... }`, closes | Trace: connection rejected, version mismatch |
| `EventPayload::BufferEdit` arrives at the service for an entity not in its registry | Silent no-op at the service; `tracing::debug` with `reason: "unowned-entity"`; the original `BufferEdit` event still lands in the core's trace via `dispatcher.process_event` (events are always traced) | Trace records the event with its emitter's provenance; no fact re-emission; no `buffer/version` bump |
| `EventPayload::BufferEdit` arrives at the service with `version != current buffer/version` | Silent no-op; `tracing::debug` with `reason: "stale-version"` or `"future-version"`, fields `{event_id, entity, emitted_version, current_version}` | Trace records the event; no fact re-emission; no `buffer/version` bump |
| `EventPayload::BufferEdit` arrives with batch-internal validation failure (out-of-bounds, mid-codepoint, intra-batch overlap, nothing-edit, swapped-endpoints, invalid-utf8) | Silent no-op; `tracing::debug` with `reason: "validation-failure-<kind>"`, fields including `edit_index` | Trace records the event; no fact re-emission; no `buffer/version` bump |
| `EventPayload::BufferEdit` arrives with empty `edits: []` and matching `version` | Silent no-op (well-formed batch with nothing to apply); `tracing::debug` with `reason: "empty-batch"` | Trace records the event; no fact re-emission; no version bump |
| Subscriber receives a `buffer-edit` `EventPayload` variant it cannot decode (e.g., a 0x03 client somehow on a 0x04 connection) | The 0x04 core would have rejected the handshake; the case cannot arise on a proper handshake. Defensive: if a future subscriber misparses, ciborium produces `Error { category: "decode", context: "unknown EventPayload variant: buffer-edit" }` and the connection closes | Trace: connection terminated |

## Versioning policy (P7 + P8)

- **Bus protocol** bumps MAJOR `0x03 → 0x04`. `Hello.protocol_version = 0x04` identifies this wire version. `CHANGELOG.md` gains a `## Bus protocol 0.4.0` entry describing the new `EventPayload::BufferEdit` variant and the supporting `TextEdit`/`Range`/`Position` types.
- **Fact-family schemas** unchanged at the shape level. `buffer/version`'s mutation consumer addition is documented as a per-family annotation under its v0.1.0 entry (no schema bump per slice-003 contract: "authority is conn/entity-keyed; the family itself is shared" — analogously here, "the schema is shape-keyed; gaining a mutation consumer is not a schema change").
- **CLI + structured output** — see `contracts/cli-surfaces.md`. MINOR additive for the `weaver` CLI (`edit` + `edit-json` subcommands).

Future compatibility notes:

- Additive `EventPayload` variants land as MAJOR until subscribers adopt skip-and-log behaviour for unknown variants. Slice 004's variant lands under a MAJOR bump; future variants targeting MINOR bumps require subscribers to tolerate unknown values first (a future-slice infrastructure concern).
- `LifecycleSignal` additions (already in slice 002) are additive with the same constraint; nothing in slice 004 changes this.

## References

- `specs/004-buffer-edit/spec.md` — user stories, functional requirements, success criteria, Clarifications 2026-04-25.
- `specs/004-buffer-edit/data-model.md` — full type definitions, validation pipeline, state-transition mapping.
- `specs/004-buffer-edit/research.md` — apply-edits algorithm, in-process inspect-lookup, ApplyError taxonomy, emitter identity, BufferRegistry refactor.
- `specs/003-buffer-service/contracts/bus-messages.md` — prior wire contract the slice extends.
- `docs/00-constitution.md §17` — multi-actor coherence (actor identity on every message).
- `docs/01-system-model.md §2.4` — Components vs Facts; the `:content` component this slice mutates.
- `docs/05-protocols.md §3.4` — actor-identity-and-delegation protocol commitment.
- `docs/07-open-questions.md §26` — component infrastructure deferral; rejection observability forward direction.
