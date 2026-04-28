# Bus Message Contracts — Slice 005

CBOR-encoded messages on the local Unix-domain-socket bus between `weaver` (core), `weaver-tui`, `weaver-git-watcher`, `weaver-buffers`, and any future client. Per L2 P5 / P7 / P8 and `docs/02-architecture.md §3.1`.

**This slice introduces a breaking wire change.** Bus protocol advances **0x04 → 0x05**. Changes:

- `EventPayload::BufferSave { entity: EntityRef, version: u64 }` is ADDED. Adjacent-tagged variant; wire tag `"buffer-save"`. No new CBOR tags — rides plain ciborium struct serialisation through the existing `EventPayload` enum machinery.
- `EventOutbound { payload, provenance, causal_parent }` struct type is ADDED as the wire-level shape of an event in flight from a producer to the listener (per FR-019..FR-024 and `research.md §1, §4`). It has **no `id` field**.
- `BusMessage<E>` becomes generic over the event-payload type. Two type aliases pin the directions:
  - `BusMessageInbound = BusMessage<EventOutbound>` — what the listener receives.
  - `BusMessageOutbound = BusMessage<Event>` — what the listener broadcasts.
  Codec is direction-typed: `read_message` returns `BusMessageInbound`; `write_message` accepts `BusMessageOutbound`. Wire byte representation is identical for non-Event variants — only the `Event` variant's payload differs between the two directions.
- The slice-004 `validate_event_envelope` ZERO-rejection on inbound `Event` is **structurally subsumed**. Pre-§28(a), the listener inspected `event.id` and rejected `EventId::ZERO`. Post-§28(a), no inbound `Event` shape exists — `BusMessageInbound::Event(_)` carries `EventOutbound`, which has no `id`. The listener allocates the stamped ID; ZERO is unreachable as a listener-allocated value because the `next_event_id` counter starts at `1`. The slice-004 consumer-side `lookup_event_for_inspect` ZERO-short-circuit is preserved per FR-024 (defence against pre-§28(a) trace entries).

Old (0x04) clients cannot connect; the handshake rejects mismatched versions with a structured error. No provenance-shape change; `ActorIdentity` (CBOR tag 1002) is unchanged from slice 002. `ActorIdentity::User` is the emitter identity stamped by `weaver save`.

## Naming conventions

Unchanged from slices 002/003/004 (Amendment 5). Identifier values on the wire are **kebab-case**; struct field names are `snake_case` in Rust / `camelCase` in JavaScript (or kebab-case via `#[serde(rename_all = "kebab-case")]` on JSON). Behavior identifiers use `/` as namespace separator; fact attributes are kebab-case with `/`-delimited namespaces (`buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`, `buffer/version`).

## Wire tagging convention

Unchanged from slice 004. Adjacent-tagged sum types (`"type"` discriminator + content field).

**Sum-type table (slice 005 delta)**:

| Enum             | Content field | Example (JSON)                                                              |
|------------------|---------------|-----------------------------------------------------------------------------|
| `BusMessage<E>`  | `payload`     | `{"type":"event","payload":{...}}` (event payload differs by direction)     |
| `ActorIdentity`  | `id` or variant-specific fields | `{"type":"user"}` (slice 004 first prod; slice 005 reused) |
| `SubscribePattern`| `pattern`    | `{"type":"family-prefix","pattern":"buffer/"}` (facts; unchanged)           |
| `EventSubscribePattern` | `pattern`     | `{"type":"payload-type","pattern":"buffer-save"}` (NEW pattern value; enum unchanged) |
| `FactValue`      | `value`       | `{"type":"bool","value":false}` (used by `buffer/dirty` re-emit on save)   |
| `EventPayload`   | `payload` (struct variants only) | `{"type":"buffer-save","payload":{"entity":42,"version":7}}` (NEW) |
| `LifecycleSignal`| —             | Unit-only; serializes as bare string: `"ready"`, `"degraded"`, etc.         |

Plain structs (`EventOutbound`) serialise as CBOR maps / JSON objects with no enum tagging — they are not sum types.

## Wire framing

Unchanged from slices 001/002/003/004. 4-byte big-endian length prefix; one frame per message; **64 KiB max** (`weaver_core::bus::codec::MAX_FRAME_SIZE`). The slice-004 `MAX_EVENT_INGEST_FRAME` constant (`MAX_FRAME_SIZE` − `RESPONSE_WRAPPER_HEADROOM` = 65 280 bytes) continues to apply for emitter-side ingest checks. `BufferSave` events are tiny (~50 bytes serialised) — the frame limit is never near; no slice-005-specific pre-check is required.

`docs/07-open-questions.md §29` (frame-headroom asymmetry on `EventInspectResponse`) remains open and unaddressed by slice 005.

## Weaver CBOR tag registry (slice 005)

| Tag number | Meaning | Encoded representation | Status |
|---|---|---|---|
| 1000 | `EntityRef` | CBOR unsigned int | existing (slice 001) |
| 1001 | `Keyword` (slash-namespaced symbol) | CBOR text string | existing (slice 001) |
| 1002 | `ActorIdentity` | CBOR map (adjacent-tagged) | existing (slice 002); **unchanged** |

**No new CBOR tags added this slice.** `EventPayload::BufferSave` and `EventOutbound` ride plain ciborium serialisation, consistent with slices 003/004 precedent.

## Connection lifecycle

Unchanged shape; the handshake carries the new protocol version.

1. **Connect**: client opens a stream to the core's socket.
2. **Handshake**: client sends `Hello { protocol_version: 0x05, client_kind: "..." }`.
   - **0x05**: core responds with `Lifecycle(Started)` then `Lifecycle(Ready)`.
   - **0x04** or lower (any mismatch): core responds `Error { category: "version-mismatch", detail: "bus protocol 0x05 required; received 0x04" }` and closes.
3. **Steady state**: client sends/receives messages per `BusMessageInbound` / `BusMessageOutbound` shapes.
4. **Disconnect**: client closes; core retracts every fact authored by the client's connection.

## Inbound vs outbound asymmetry

The §28(a) wire-shape change introduces a single point of asymmetry between what producers send and what subscribers receive. Document explicitly:

| Direction | Type | `Event` shape | ID source |
|---|---|---|---|
| Producer → listener (inbound) | `BusMessageInbound::Event(EventOutbound)` | `{payload, provenance, causal_parent}` (no `id`) | none (listener allocates) |
| Listener → subscriber (broadcast) | `BusMessageOutbound::Event(Event)` | `{id, payload, provenance, causal_parent}` | core-stamped on accept |

Producers MUST construct `EventOutbound` (no manual `EventId` minting). The listener:
1. Receives `BusMessageInbound::Event(outbound)`.
2. Allocates a stamped `EventId` from `TraceStore::next_event_id` (counter starts at `1`; ZERO never allocated).
3. Constructs `Event::from_outbound(stamped_id, outbound)`.
4. Inserts into `TraceStore::by_event` (keyed by stamped ID).
5. Broadcasts `BusMessageOutbound::Event(stamped_event)` to subscribers.

A producer that attempts to send a wire-level `Event` (with `id`) on the inbound channel cannot succeed — the codec's `read_message` returns `BusMessageInbound` typed with `EventOutbound`; deserialisation as `EventOutbound` rejects the `id` field's presence (extra-field strictness via serde derive default OR a `deny_unknown_fields` attribute, per `research.md §4`). The codec returns a structured decode error to the producer; SC-506 verifies this end-to-end.

## `EventPayload::BufferSave` — full wire example

**JSON** (kebab-case, adjacent-tagged):

```json
{
  "type": "buffer-save",
  "payload": {
    "entity": 4611686018427387946,
    "version": 7
  }
}
```

**Wrapped in `EventOutbound`** (producer-side):

```json
{
  "payload": {
    "type": "buffer-save",
    "payload": {"entity": 4611686018427387946, "version": 7}
  },
  "provenance": {
    "source": {"type": "user"},
    "timestamp-ns": 1714217040123456789,
    "causal-parent": null
  },
  "causal-parent": null
}
```

**Wrapped in `Event`** (listener-broadcast side, after stamping):

```json
{
  "id": 4216,
  "payload": {
    "type": "buffer-save",
    "payload": {"entity": 4611686018427387946, "version": 7}
  },
  "provenance": {
    "source": {"type": "user"},
    "timestamp-ns": 1714217040123456789,
    "causal-parent": null
  },
  "causal-parent": null
}
```

**`BusMessageInbound`-wrapped** (full wire frame producer sends):

```json
{
  "type": "event",
  "payload": <the EventOutbound shape above>
}
```

**`BusMessageOutbound`-wrapped** (full wire frame subscribers receive):

```json
{
  "type": "event",
  "payload": <the Event shape above>
}
```

## Failure modes (slice 005 wire-protocol failures)

Beyond the slice-004 envelope-validation surface, slice 005 introduces:

- **Decode error: producer sent `Event { id, .. }` shape on inbound channel.** Codec returns `Err(CodecError::FrameDecode(...))` per the `EventOutbound` deserialisation strictness. The connection receives `BusMessage::Error { category: "decode", detail: "expected EventOutbound shape on inbound; received Event-with-id" }` and closes. SC-506.
- **Stale-version drop.** Listener accepts the `EventOutbound`, stamps it, dispatches to `weaver-buffers`. The dispatcher's R2 step (version handshake) fails; emits `WEAVER-SAVE-002` at debug; no fact re-emission. The CLI emitter cannot detect this (silent drop per FR-013).
- **Inode-mismatch refusal / path-missing refusal.** Listener accepts; dispatcher's R4 step fails; emits `WEAVER-SAVE-005` / `WEAVER-SAVE-006` at warn; no fact re-emission. CLI cannot detect.
- **Tempfile / rename I/O failure.** Listener accepts; dispatcher's R5 step fails; emits `WEAVER-SAVE-003` / `WEAVER-SAVE-004` at error; no fact re-emission. CLI cannot detect.

All service-side failure modes are stderr-only per FR-018; the bus does not surface a per-event rejection frame for save (consistent with slice-004 lossy-class semantics for events).

## Subscription patterns

Unchanged from slice 004. `BusMessage::SubscribeEvents(EventSubscribePattern::PayloadType("buffer-save"))` enables a subscriber to receive every accepted `BufferSave` event. `weaver-buffers` adds this pattern alongside its existing `payload-type=buffer-edit` and `payload-type=buffer-open` subscriptions.

## §28(a) trace-store implications

`TraceStore::by_event` is keyed by stamped `EventId`. Under §28(a):

- Every accepted event has a unique `EventId`. `by_event::insert` is collision-free for any pair of distinct accepted events; last-writer-wins semantics on insert no longer have hazardous user-visible consequences. (The map's `insert` may still mechanically overwrite if called twice with the same key, but that key is allocated by atomic-fetch-add and is provably unique.)
- `find_event(id)` returns the event indexed at `id`; under §28(a), this is the unique event the listener stamped at that id. SC-505 verifies 100% walkback resolution under multi-producer stress.
- Pre-§28(a) trace entries (which would only exist in long-running deployments that bridged the upgrade) may have non-monotonic / ZERO IDs; the slice-004 `lookup_event_for_inspect` ZERO-short-circuit (FR-024) preserves correctness on those.

---

*Phase 1 — bus-messages.md complete. CLI surfaces and quickstart follow.*
