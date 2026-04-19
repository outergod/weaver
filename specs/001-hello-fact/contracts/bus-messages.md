# Bus Message Contracts

CBOR-encoded messages on the local Unix-domain-socket bus between `weaver` (core) and `weaver-tui` (and any future client). Per L2 P5 and arch §3.1.

## Wire framing

```
┌─────────────────┬───────────────────────────────────────────┐
│ length (u32 BE) │ CBOR-encoded BusMessage                   │
└─────────────────┴───────────────────────────────────────────┘
```

- 4-byte big-endian unsigned length prefix; payload immediately follows.
- Length is in bytes; max payload in this slice: 64 KiB. Larger payloads close the connection with a structured error.
- One frame per message; no multi-frame messages in this slice.

## Weaver CBOR tag registry (initial)

| Tag number | Meaning | Encoded representation | Status |
|---|---|---|---|
| 1000 | `EntityRef` | CBOR unsigned int | initial in this slice |
| 1001 | `Keyword` (slash-namespaced symbol like `buffer/dirty`) | CBOR text string | initial in this slice |

Tag numbers are a public surface per L2 P7. Subsequent additions land via `CHANGELOG.md` entries with the bus-protocol-version bump.

## Connection lifecycle

1. **Connect**: client opens a stream connection to the core's socket.
2. **Handshake**: client sends `Hello { protocol_version: 0x01, client_kind: "..." }`. Core responds with either `Lifecycle(Ready)` or `Error { category: "version_mismatch", ... }` followed by close.
3. **Subscribe / interact**: client may send `Subscribe(...)`, `Event(...)`, `InspectRequest { ... }`. Core sends `FactAssert`, `FactRetract`, `Lifecycle`, and `InspectResponse` as appropriate.
4. **Disconnect**: either side may close the stream; core emits a trace entry recording the disconnection.

## Message-by-message contract

### `Hello`

**Direction**: client → core, exactly once at connection start.

```
Hello {
    protocol_version: u8,        // 0x01
    client_kind: String          // free-form; "tui", "cli", "test", etc.
}
```

Sending any other message before `Hello` triggers `Error { category: "protocol", detail: "expected Hello" }` and connection close.

### `Event` (lossy delivery class)

**Direction**: bidirectional.

```
Event {
    id: EventId,
    name: String,                // family/verb, e.g., "buffer/edited"
    target: Option<EntityRef>,
    payload: EventPayload,       // BufferEdited | BufferCleaned (this slice)
    provenance: Provenance
}
```

- Subject to per-subscriber bounded queue with `drop-oldest` per arch §3.1.
- No sequence guarantees beyond `EventId` monotonicity per producer.
- No replay on reconnect.

### `FactAssert` (authoritative delivery class)

**Direction**: core → subscribers.

```
FactAssert(Fact)
```

- Per-publisher monotonic sequence numbers; subscribers detect gaps.
- On reconnect, subscribers receive the current snapshot of subscribed fact families followed by missed deltas.
- `block-with-timeout` back-pressure (bounded; never `block-forever`).

### `FactRetract` (authoritative delivery class)

**Direction**: core → subscribers.

```
FactRetract {
    key: FactKey,
    provenance: Provenance       // who and why; causal_parent points to the retracting event
}
```

Same delivery semantics as `FactAssert`. L2 P20 (retraction first-class) — every assert has a symmetric retract path.

### `Subscribe` / `SubscribeAck`

**Direction**: client → core (`Subscribe`); core → client (`SubscribeAck`).

```
Subscribe(SubscribePattern)
SubscribeAck { sequence: u64 }   // starting sequence; subsequent FactAssert/FactRetract on this subscription will be ≥ this number
```

The `sequence` lets subscribers detect gaps if the queue's `drop-oldest` policy were ever incorrectly applied to the authoritative class (defensive — should never trigger).

### `InspectRequest` / `InspectResponse` (authoritative delivery class)

**Direction**: client → core (`InspectRequest`); core → client (`InspectResponse`).

```
InspectRequest {
    request_id: u64,             // client-generated; correlates response
    fact: FactKey
}

InspectResponse {
    request_id: u64,             // echo of request_id
    result: Result<InspectionDetail, InspectionError>
}

InspectionDetail {
    source_event: EventId,
    asserting_behavior: BehaviorId,
    asserted_at_ns: u64,
    trace_sequence: u64
}

InspectionError = enum {
    FactNotFound,
    NoProvenance                 // P11 violation; defensive
}
```

This is the bus-level inspection per FR-008 / amended L2 P5. The CLI's `weaver inspect <fact-ref>` is a thin wrapper. Future agent services will issue this same request without the CLI in the loop.

### `Lifecycle` (authoritative)

**Direction**: core → subscribers (broadcast on state change).

```
Lifecycle = enum { Started, Ready, Stopped }
```

Initial subscription receives the current state immediately after `SubscribeAck` if not already known.

### `Error` (authoritative)

**Direction**: bidirectional.

```
Error {
    category: String,            // "protocol" | "version_mismatch" | "behavior_failure" | ...
    detail: String,
    context: Option<String>      // e.g., the offending message's request_id
}
```

Receiving an `Error` does not by itself close the connection — the sender of the error decides whether to close.

## Failure modes (FR-009/FR-010 + P16 alignment)

| Failure | Wire behavior | Trace consequence |
|---|---|---|
| Client sends non-`Hello` first | Core sends `Error { category: "protocol", ... }`, closes | TraceEntry::Lifecycle(Stopped subscription) |
| Protocol version mismatch | Core sends `Error { category: "version_mismatch", ... }`, closes | TraceEntry: connection rejected |
| Frame length > 64 KiB | Either side sends `Error { category: "frame_too_large", ... }`, closes | TraceEntry: connection terminated |
| CBOR decode failure | Receiver sends `Error { category: "decode", ... }`, closes | TraceEntry |
| Behavior firing error during event handling | Core continues; emits `TraceEntry::BehaviorFired { error: Some(...) }`; subscribers are not notified by default in this slice | Trace only |
| Core process exits | Connection drops; client observes EOF; client surfaces unavailability per FR-009 | (No trace; core is gone) |

## Versioning policy (L2 P7 + P8)

Bus protocol surface starts at `0x01` (v0.1.0). Changes:

- **PATCH** (e.g., 0.1.0 → 0.1.1): purely additive `Error.category` strings or new `LifecycleSignal` variants that older clients can ignore.
- **MINOR** (0.1.0 → 0.2.0): new `BusMessage` variants, new tag-registry entries (additive). Old clients ignore unknown variants per CBOR's tag-skip semantics; `Hello.protocol_version` may stay at `0x01`.
- **MAJOR** (0.x → 1.0): wire-incompatible changes — `Hello.protocol_version` advances. Mismatches close cleanly.

`CHANGELOG.md` records every bus-protocol change per L2 P8.
