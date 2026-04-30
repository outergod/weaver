# Bus Message Contracts — Slice 005

CBOR-encoded messages on the local Unix-domain-socket bus between `weaver` (core), `weaver-tui`, `weaver-git-watcher`, `weaver-buffers`, and any future client. Per L2 P5 / P7 / P8 and `docs/02-architecture.md §3.1`.

**This slice introduces a breaking wire change.** Bus protocol advances **0x04 → 0x05**. Changes:

- `EventPayload::BufferSave { entity: EntityRef, version: u64 }` is ADDED. Adjacent-tagged variant; wire tag `"buffer-save"`. No new CBOR tags — rides plain ciborium struct serialisation through the existing `EventPayload` enum machinery.
- `EventId` wire shape changes from `u64` (8-byte CBOR unsigned int) to `Uuid` (16-byte CBOR byte-string) per FR-019..FR-024 and `research.md §5, §12`. `EventId(Uuid)` is producer-minted as **UUIDv8** with the producer's hashed identity in the high 58 bits of the custom payload (Service `instance_id` for Service producers; per-process UUIDv4 for non-Service producers, hashed via `std::collections::hash_map::DefaultHasher` SipHash) and nanoseconds (process-monotonic or wall-clock; producer's local invariant) in the low 64 bits. Cross-producer collision is structurally impossible — distinct producers occupy disjoint 58-bit-prefix namespaces.
- `Event` envelope shape is unchanged from slice-001 canonical `{ id, name, target, payload, provenance }`. There is no `EventOutbound` / `Event` envelope split; producers and listener exchange the same `Event` type. `BusMessage` is unchanged from its slice-004 non-generic shape — no `BusMessage<E>` generic refactor, no direction-typed codec siblings.
- The slice-004 `validate_event_envelope` ZERO-rejection at the listener is **preserved** as `EventId::nil()` rejection (the all-zero `Uuid` value, semantically equivalent to slice-004's `EventId::ZERO` retargeted at the new wire shape). The `lookup_event_for_inspect` short-circuit (slice-004 PR #11 commit `f0112d4`) is preserved against `EventId::nil()` walkbacks per FR-024 (defence-in-depth against pre-§28(a) trace entries).
- **Carry-forward deferral**: listener-side **prefix-vs-provenance verification** (catching a malicious producer that mints UUIDv8s under another producer's prefix — the `ActorIdentity::Service::instance_id` of a different service, say) is **DEFERRED** to slice 006 alongside FR-029, the unauthenticated-edit/save-channel close-out. Both share the same hazard class (the bus does not yet bind `ActorIdentity` to the connection at handshake time, so a producer can spoof identity at the wire layer). Within-slice trust assumption: well-behaved producers mint under their own prefix only.

Old (0x04) clients cannot connect; the handshake rejects mismatched versions with a structured error. No provenance-shape change; `ActorIdentity` (CBOR tag 1002) is unchanged from slice 002. `ActorIdentity::User` is the emitter identity stamped by `weaver save`.

## Naming conventions

Unchanged from slices 002/003/004 (Amendment 5). Identifier values on the wire are **kebab-case**; struct field names are `snake_case` in Rust / `camelCase` in JavaScript (or kebab-case via `#[serde(rename_all = "kebab-case")]` on JSON). Behavior identifiers use `/` as namespace separator; fact attributes are kebab-case with `/`-delimited namespaces (`buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`, `buffer/version`).

## Wire tagging convention

Unchanged from slice 004. Adjacent-tagged sum types (`"type"` discriminator + content field).

**Sum-type table (slice 005 delta)**:

| Enum             | Content field | Example (JSON)                                                              |
|------------------|---------------|-----------------------------------------------------------------------------|
| `BusMessage`     | `payload`     | `{"type":"event","payload":{...}}` (unchanged from slice 004 — no generic refactor)|
| `ActorIdentity`  | `id` or variant-specific fields | `{"type":"user"}` (slice 004 first prod; slice 005 reused) |
| `SubscribePattern`| `pattern`    | `{"type":"family-prefix","pattern":"buffer/"}` (facts; unchanged)           |
| `EventSubscribePattern` | `pattern`     | `{"type":"payload-type","pattern":"buffer-save"}` (NEW pattern value; enum unchanged) |
| `FactValue`      | `value`       | `{"type":"bool","value":false}` (used by `buffer/dirty` re-emit on save)   |
| `EventPayload`   | `payload` (struct variants only) | `{"type":"buffer-save","payload":{"entity":42,"version":7}}` (NEW) |
| `LifecycleSignal`| —             | Unit-only; serializes as bare string: `"ready"`, `"degraded"`, etc.         |

## Wire framing

Unchanged from slices 001/002/003/004. 4-byte big-endian length prefix; one frame per message; **64 KiB max** (`weaver_core::bus::codec::MAX_FRAME_SIZE`). The slice-004 `MAX_EVENT_INGEST_FRAME` constant (`MAX_FRAME_SIZE` − `RESPONSE_WRAPPER_HEADROOM` = 65 280 bytes) continues to apply for emitter-side ingest checks. `BufferSave` events are tiny (~50 bytes serialised) — the frame limit is never near; no slice-005-specific pre-check is required.

`docs/07-open-questions.md §29` (frame-headroom asymmetry on `EventInspectResponse`) remains open and unaddressed by slice 005.

## Weaver CBOR tag registry (slice 005)

| Tag number | Meaning | Encoded representation | Status |
|---|---|---|---|
| 1000 | `EntityRef` | CBOR unsigned int | existing (slice 001) |
| 1001 | `Keyword` (slash-namespaced symbol) | CBOR text string | existing (slice 001) |
| 1002 | `ActorIdentity` | CBOR map (adjacent-tagged) | existing (slice 002); **unchanged** |

**No new CBOR tags added this slice.** `EventPayload::BufferSave` rides plain ciborium adjacent-tag serialisation through the existing `EventPayload` enum machinery; `EventId(Uuid)` rides the `uuid` crate's CBOR-byte-string serde derive (16-byte byte-string). Consistent with slices 003/004 precedent.

## Connection lifecycle

Unchanged shape; the handshake carries the new protocol version.

1. **Connect**: client opens a stream to the core's socket.
2. **Handshake**: client sends `Hello { protocol_version: 0x05, client_kind: "..." }`.
   - **0x05**: core responds with `Lifecycle(Started)` then `Lifecycle(Ready)`.
   - **0x04** or lower (any mismatch): core responds `Error { category: "version-mismatch", detail: "bus protocol 0x05 required; received 0x04" }` and closes.
3. **Steady state**: client sends/receives messages per the (non-generic, slice-004-shape) `BusMessage` enum.
4. **Disconnect**: client closes; core retracts every fact authored by the client's connection.

## UUIDv8 producer-prefix convention

Under the §28(a) UUIDv8 re-derivation, every producer mints `EventId` locally with its own hashed-identity prefix. There is NO inbound/outbound asymmetry on the `Event` carrier — producers and listener exchange the same `Event` shape (slice-001 canonical `{ id, name, target, payload, provenance }`). The wire-layer change is at the `EventId` payload (8 bytes → 16 bytes; UUIDv8 producer-minted) and nowhere else.

**Producer-prefix derivation** (per FR-019, `research.md §5, §12`):

| Producer kind | Prefix source | Prefix lifetime |
|---|---|---|
| `ActorIdentity::Service` (e.g., `weaver-buffers`, `weaver-git-watcher`, future Service producers) | `hash_to_58(&actor_identity.instance_id)` — SipHash via `std::collections::hash_map::DefaultHasher`; `instance_id` is the slice-002 UUIDv4 generated at process start. | Per-process; producer restart yields a fresh prefix. Acceptable because in-memory traces don't survive listener restart. |
| `ActorIdentity::User` (CLI emitters: `weaver edit`, `weaver edit-json`, `weaver save`) | `hash_to_58(&per_process_uuid_v4)` — each producer process generates a UUIDv4 at startup (`OnceLock<Uuid>`), hashed via the same SipHash mechanism. The per-process UUIDv4 is internal to the producer (NOT part of the wire `ActorIdentity::User` shape — that variant stays unit-shaped per slice-002/004). | Per-process; same as Service. |
| Future `ActorIdentity::Agent` (slice 006) | TBD; same scheme as User (per-process UUIDv4 hashed) is the natural default unless slice-006 introduces an Agent-specific instance_id field. | Per-process. |

**UUIDv8 bit layout** (RFC 9562):
- High 58 bits of custom payload: producer prefix (above).
- Version nibble (byte 6 high nibble): `0x8` (UUIDv8 marker).
- Variant bits (byte 8 high two bits): `0b10` (RFC 4122 variant).
- Remaining ~62 bits of custom payload: `time_or_counter` (process-monotonic or wall-clock nanoseconds; producer's local invariant). 2 bits dropped to fit the variant marker.

**Mint helper**: `EventId::mint_v8(producer_prefix_58: u64, time_or_counter: u64) -> Self` (in `core/src/types/ids.rs`).

**Prefix recovery for display**: `EventId::extract_prefix(&self) -> u64` recovers the 58-bit producer prefix from a UUIDv8 EventId. Used by the TUI and `weaver inspect` passive-cache layer (slice-005 tasks T-A3 + T-A4) to render `EventId(<friendly_name>/<short-suffix>)` for known prefixes; full UUID hex via `--output=json`.

**Listener-side enforcement and deferral**:

The listener performs the slice-004 envelope-validation pipeline unchanged at the structural level:
1. Codec parses the inbound `Event` (including the 16-byte UUID `id`); strict-parsing via the `uuid` crate rejects malformed UUID payloads — see SC-506 below.
2. `validate_event_envelope` rejects `event.id == EventId::nil()` (semantically unchanged from slice-004's ZERO-rejection, retargeted at the new wire shape).
3. `validate_event_envelope` enforces envelope/payload entity-match (slice-004 origin) on `BufferEdit` AND `BufferSave` (the new variant).
4. **DEFERRED to slice 006**: prefix-vs-provenance verification — the listener does NOT yet check that `extract_prefix(event.id)` matches the connection's authenticated `ActorIdentity`'s expected prefix. Joins FR-029's deferral.

A producer that attempts to send a wire-level `Event` whose `id` field is malformed UUID bytes (wrong version nibble — not `0x8`; wrong variant bits; raw bytes that fail UUID parsing) gets rejected at the codec layer via the `uuid` crate's strict-parsing path. The codec returns a structured decode error to the producer; the connection receives `BusMessage::Error { category: "decode", .. }` and closes; the trace contains no entry for the rejected event. SC-506 verifies this end-to-end.

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

**Wrapped in `Event`** (producer-minted UUIDv8 `id`; same shape on the wire whether producer→listener or listener→subscriber):

```json
{
  "id": "01863f4e-9c2a-8000-8421-c5d2e4f6a7b8",
  "name": "buffer/save",
  "target": 4611686018427387946,
  "payload": {
    "type": "buffer-save",
    "payload": {"entity": 4611686018427387946, "version": 7}
  },
  "provenance": {
    "source": {"type": "user"},
    "timestamp_ns": 1714217040123456789,
    "causal_parent": null
  }
}
```

The `id` field renders as a UUID string (RFC 4122 hyphenated lowercase hex) in JSON. In CBOR, the `id` field is a 16-byte byte-string. The version nibble (`8` in the example above, after the third hyphen) marks this as a UUIDv8; the variant bits (`8` in the leading hex of the fourth group) mark it as RFC-4122-variant. The high 58 bits encode the producer's hashed-identity prefix; the low 64 bits encode nanoseconds (in this example, the timestamp `1714217040123456789` is recoverable from the low bits — the exact bit layout is in `research.md §5`).

**`BusMessage`-wrapped** (full wire frame, producer or subscriber direction; identical shape):

```json
{
  "type": "event",
  "payload": <the Event shape above>
}
```

## Failure modes (slice 005 wire-protocol failures)

Beyond the slice-004 envelope-validation surface, slice 005 introduces:

- **Decode error: producer sent `Event` whose `id` field is malformed UUID bytes (wrong version nibble — not `0x8` for UUIDv8; wrong variant bits; raw bytes that fail UUID parsing entirely).** Codec returns `Err(CodecError::FrameDecode(...))` via the `uuid` crate's strict-parsing path. The connection receives `BusMessage::Error { category: "decode", detail: "malformed UUID in event.id" }` and closes. SC-506. Note: producer-prefix-vs-provenance verification (catching identity spoofing — a producer mints UUIDv8s under another producer's hashed-instance-id prefix) is DEFERRED to slice 006 alongside FR-029.
- **`EventId::nil()` rejection.** A producer that emits an event with `id == EventId::nil()` (the all-zero UUID) is rejected by `validate_event_envelope` at the listener (slice-004 origin, retargeted at the new wire shape per FR-024). The connection receives `BusMessage::Error { category: "invalid-event-envelope", detail: "EventId::nil() is reserved for 'no causal parent' lookups" }`.
- **Stale-version drop.** Listener accepts the `Event`, dispatches to `weaver-buffers`. The dispatcher's R2 step (version handshake) fails; emits `WEAVER-SAVE-002` at debug; no fact re-emission. The CLI emitter cannot detect this (silent drop per FR-013).
- **Inode-mismatch refusal / path-missing refusal.** Listener accepts; dispatcher's R4 step fails; emits `WEAVER-SAVE-005` / `WEAVER-SAVE-006` at warn; no fact re-emission. CLI cannot detect.
- **Tempfile / rename I/O failure.** Listener accepts; dispatcher's R5 step fails; emits `WEAVER-SAVE-003` / `WEAVER-SAVE-004` at error; no fact re-emission. CLI cannot detect.

All service-side failure modes are stderr-only per FR-018; the bus does not surface a per-event rejection frame for save (consistent with slice-004 lossy-class semantics for events).

## Subscription patterns

Unchanged from slice 004. `BusMessage::SubscribeEvents(EventSubscribePattern::PayloadType("buffer-save"))` enables a subscriber to receive every accepted `BufferSave` event. `weaver-buffers` adds this pattern alongside its existing `payload-type=buffer-edit` and `payload-type=buffer-open` subscriptions.

## §28(a) trace-store implications

`TraceStore::by_event` is keyed by `EventId(Uuid)`. Under §28(a)'s UUIDv8 re-derivation:

- Every accepted event has a unique `EventId` because distinct producers occupy disjoint 58-bit-prefix namespaces and within-producer monotonicity is the producer's local invariant. `by_event::insert` is collision-free for any pair of distinct accepted events. (The map's `insert` may still mechanically overwrite if called twice with the same key, but that key is producer-minted from a partitioned namespace + local monotonicity and is provably unique under the within-slice trust assumption.)
- `find_event(id)` returns the event indexed at `id`; under §28(a) this is the unique event minted at that id. SC-505 verifies 100% walkback resolution + cross-producer collision-freedom under multi-producer stress.
- Pre-§28(a) trace entries do not coexist in the same trace as post-§28(a) entries because the trace is in-memory only and does not survive listener restart; the wire bump is a clean break. The slice-004 `lookup_event_for_inspect` short-circuit (FR-024) is preserved against `EventId::nil()` walkbacks as defence-in-depth.

---

*Phase 1 — bus-messages.md complete. CLI surfaces and quickstart follow.*
