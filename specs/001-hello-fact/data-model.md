# Data Model: Hello, fact

Domain types for the Hello-fact slice. Types live in `core/src/lib.rs`; both `core` and `tui` consume them. All types derive `serde::Serialize + Deserialize` for CBOR (bus) and JSON (CLI output) round-trips per L2 P5.

> **Implementation note — ECS-library decision deferred.** The fact-space storage is intentionally hidden behind a narrow `FactStore` trait (`assert / retract / query / subscribe / snapshot`) so the choice between hand-rolled archetype storage, `bevy_ecs`, `hecs`, `flecs-rs`, or a custom ECS can be made in a later slice when fact families and behavior counts justify the evaluation. For Hello-fact, a `HashMap<FactKey, Fact>` impl behind the trait is sufficient. See `research.md` §13 for the full decision and revisit triggers. The types defined below are storage-agnostic — they are the *contract surface*, not the storage layout.

## Type catalog

### EntityRef

Opaque, addressable reference to an entity. No intrinsic type per L1 §3 (Entities Are Untyped).

```text
EntityRef(u64)            // newtype around a monotonic counter (slice 1 — single core)
                          // CBOR tag: 1000 (Weaver entity-ref)
```

Identity is **deterministic for the synthetic buffer** in this slice: the buffer entity is `EntityRef(1)`. (Future slices with persistent entities will use a stable scheme — e.g., content-addressed or path-derived hashes — but Hello-fact has only one buffer.)

### FactKey

The `(entity, attribute)` tuple that identifies a single fact in the fact space. A given key holds at most one value at a time (assertion replaces, retraction removes).

```text
FactKey {
    entity: EntityRef,
    attribute: String       // e.g., "buffer/dirty"
}
```

`attribute` is a slash-namespaced name; the namespace before the first `/` identifies the fact family (`buffer`).

### Fact

An asserted fact in the fact space.

```text
Fact {
    key: FactKey,
    value: FactValue,           // sum type: Bool(bool), String(String), Int(i64), Null
    provenance: Provenance
}
```

For Hello-fact, the only fact in scope is `buffer/dirty` whose value is `Bool(true)` (presence implies dirty; retraction means clean).

### Provenance

Required metadata on every fact, event, and trace entry per L2 P11.

```text
Provenance {
    source: SourceId,           // who produced this — see below
    timestamp_ns: u64,          // monotonic nanoseconds since process start
    causal_parent: Option<EventId>  // the event that caused this, if any
}

SourceId = enum {
    Core,                       // produced by the core itself
    Behavior(BehaviorId),       // produced by a registered behavior
    Tui,                        // produced by the TUI process
    External(String),           // future: services, agents
}
```

### EventId, BehaviorId

Strongly-typed identifiers, distinct from each other and from `EntityRef`.

```text
EventId(u64)       // monotonic per producer; unique across the lifetime of the bus connection
BehaviorId(String) // human-readable; e.g., "core/dirty-tracking"
```

### Event

A transient bus message indicating something happened. Lossy delivery class per arch §3.1.

```text
Event {
    id: EventId,
    name: String,               // e.g., "buffer/edited"
    target: Option<EntityRef>,  // the entity the event is about
    payload: EventPayload,
    provenance: Provenance
}

EventPayload = enum {
    BufferEdited,               // no fields in this slice
    BufferCleaned,              // no fields in this slice
}
```

`name` is structured: `<family>/<verb>`. The enum + name pairing is intentional — the enum is the typed Rust face, the string is the wire-stable identifier per L2 P7.

### BusMessage

The top-level enum carried over the bus. CBOR-encoded.

```text
BusMessage = enum {
    Hello(HelloMsg),                // handshake; carries protocol version
    Event(Event),                   // lossy
    FactAssert(Fact),               // authoritative
    FactRetract { key: FactKey, provenance: Provenance },  // authoritative
    Subscribe(SubscribePattern),    // request (subscribe to fact updates)
    SubscribeAck { sequence: u64 }, // response: starting sequence number
    InspectRequest { request_id: u64, fact: FactKey },     // FR-008 bus request
    InspectResponse {
        request_id: u64,
        result: Result<InspectionDetail, InspectionError>
    },
    Lifecycle(LifecycleSignal),     // authoritative
    Error(ErrorMsg)                 // authoritative
}

HelloMsg {
    protocol_version: u8,           // 0x01 in this slice
    client_kind: String             // e.g., "tui", "cli", "agent"
}

SubscribePattern = enum {
    AllFacts,                       // subscribe to every assert/retract
    FamilyPrefix(String),           // e.g., "buffer/" — only buffer facts
}

LifecycleSignal = enum {
    Started,
    Ready,
    Stopped
}

ErrorMsg {
    category: String,
    detail: String,
    context: Option<String>
}

InspectionDetail {
    source_event: EventId,
    asserting_behavior: BehaviorId,
    asserted_at_ns: u64,
    trace_sequence: u64
}

InspectionError = enum {
    FactNotFound,
    NoProvenance        // shouldn't happen given P11; defensive
}
```

### TraceEntry

Append-only log entry; in-memory only for this slice.

```text
TraceEntry {
    sequence: u64,                  // monotonic across the trace
    timestamp_ns: u64,
    payload: TracePayload
}

TracePayload = enum {
    Event { event: Event },
    FactAsserted { fact: Fact },
    FactRetracted { key: FactKey, provenance: Provenance },
    BehaviorFired {
        behavior: BehaviorId,
        triggering_event: EventId,
        asserted: Vec<FactKey>,
        retracted: Vec<FactKey>,
        error: Option<String>
    },
    Lifecycle(LifecycleSignal)
}
```

The reverse causal index (arch §10.1 — `O(path length)` traversal) is built incrementally as entries append: maps from `EventId` and `FactKey` back to the producing trace sequence.

## Relationships

```text
Event  -- causes -->  BehaviorFired  -- asserts -->  Fact
                              |
                              +-- retracts -->  FactKey

Fact.provenance.causal_parent  -->  Event.id           (the event that caused the fact)
Event.provenance.causal_parent -->  Event.id (parent)  (for derived events; none in this slice)
TraceEntry.sequence            <--  reverse index by EventId / FactKey
```

## Validation rules

| Rule | Origin | Test type |
|---|---|---|
| `Provenance.source` is never `External("")` | L2 P11 | Property test on `Provenance::new` |
| `Provenance.timestamp_ns` is monotonic per `SourceId` | L2 P11 | Property test on bus publish path |
| `EventId` is monotonic per `SourceId` | arch §3.1 (authoritative class sequence) | Property test |
| `FactAssert` and `FactRetract` round-trip preserves `FactKey` | L2 P20 (retraction first-class) | Property test |
| Asserted fact is observable via subscription within 100 ms | spec SC-001 | Scenario test (timing-aware) |
| Retracted fact is no longer observable via query within 100 ms | spec SC-001 (extended to retraction) | Scenario test |
| `InspectRequest` for a non-existent fact returns `InspectionError::FactNotFound`, never panics | L2 P3 + L2 P6 | Scenario test |
| Bus protocol version mismatch on `Hello` triggers a structured `Error` and connection close | research §11 | Scenario test |

## State transitions

Synthetic buffer entity `EntityRef(1)`:

```
   ┌─────────────────────────┐
   │ no buffer/dirty fact    │   (initial state; equivalent to "clean")
   └────────────┬────────────┘
                │ Event(name="buffer/edited", target=EntityRef(1))
                │  → behavior::dirty_tracking fires
                │  → FactAssert(buffer/dirty=true)
                ▼
   ┌─────────────────────────┐
   │ buffer/dirty = true     │   (TUI shows dirty indicator)
   └────────────┬────────────┘
                │ Event(name="buffer/cleaned", target=EntityRef(1))
                │  → behavior::dirty_tracking fires
                │  → FactRetract(buffer/dirty)
                ▼
   ┌─────────────────────────┐
   │ no buffer/dirty fact    │   (TUI removes dirty indicator)
   └─────────────────────────┘
```

Re-edits while already dirty are idempotent (assert of an already-asserted fact updates `provenance` but does not change `value`). This is *the* fact's update model in Hello-fact; richer semantics (e.g., dirty count, last-edit timestamp as a separate fact) belong to a later slice.

## Out of scope (data model)

- Buffer content as a `Component` (system-model §2.4) — not exercised in this slice.
- Action entities — no actions; `simulate-edit` and `simulate-clean` are direct event publications, not action invocations.
- Multi-buffer fact-space queries — only one buffer.
- Workspace, project, file-path facts — not introduced.
- User-scratch facts — no Steel; no scratch lane.
