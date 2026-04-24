# Bus Message Contracts — Slice 003

CBOR-encoded messages on the local Unix-domain-socket bus between `weaver` (core), `weaver-tui`, `weaver-git-watcher`, the new `weaver-buffers`, and any future client. Per L2 P5 / P7 / P8 and `docs/02-architecture.md §3.1`.

**This slice introduces a breaking wire change.** Bus protocol advances **0x02 → 0x03**. Changes:
- `EventPayload::BufferEdited` and `EventPayload::BufferCleaned` are REMOVED.
- `EventPayload::BufferOpen { path: String }` is ADDED.
- `FactValue::U64(u64)` is ADDED (additive variant landing under the MAJOR bump).

Old (0x02) clients cannot connect; the handshake rejects mismatched versions with a structured error. No provenance-shape change; `ActorIdentity` (CBOR tag 1002) is unchanged from slice 002.

## Naming conventions

Unchanged from slice 002 (Amendment 5). Identifier values on the wire are **kebab-case**; struct field names are `snake_case` in Rust / `camelCase` in JavaScript. Behavior identifiers use `/` as namespace separator; fact attributes are kebab-case with `/`-delimited namespaces (`buffer/path`, `buffer/byte-size`, `buffer/dirty`, `buffer/observable`).

## Wire tagging convention

Unchanged from slice 002. Adjacent-tagged sum types (`"type"` discriminator + content field).

**Sum-type table (slice 003 delta)**:

| Enum             | Content field | Example (JSON)                                                              |
|------------------|---------------|-----------------------------------------------------------------------------|
| `BusMessage`     | `payload`     | `{"type":"fact-assert","payload":{...}}`                                    |
| `ActorIdentity`  | `id` or variant-specific fields | `{"type":"service","service-id":"weaver-buffers","instance-id":"<uuid>"}` |
| `SubscribePattern`| `pattern`    | `{"type":"family-prefix","pattern":"buffer/"}`                              |
| `FactValue`      | `value`       | `{"type":"bool","value":true}` / `{"type":"string","value":"/path"}` / **`{"type":"u64","value":12345}` (NEW)** |
| `EventPayload`   | `payload` (struct variants only) | `{"type":"buffer-open","payload":{"path":"/home/alex/file.txt"}}` (NEW) |
| `LifecycleSignal`| —             | Unit-only; serializes as bare string: `"ready"`, `"degraded"`, etc.         |

## Wire framing

Unchanged from slices 001/002. 4-byte big-endian length prefix; one frame per message; 64 KiB max.

## Weaver CBOR tag registry (slice 003)

| Tag number | Meaning | Encoded representation | Status |
|---|---|---|---|
| 1000 | `EntityRef` | CBOR unsigned int | existing (slice 001) |
| 1001 | `Keyword` (slash-namespaced symbol) | CBOR text string | existing (slice 001) |
| 1002 | `ActorIdentity` | CBOR map (adjacent-tagged) | existing (slice 002); **unchanged** |

**No new CBOR tags added this slice.** The new `FactValue::U64` variant serializes via the existing adjacent-tagged enum machinery; it does NOT require its own CBOR tag.

## Connection lifecycle

Unchanged shape; the handshake carries the new protocol version.

1. **Connect**: client opens a stream to the core's socket.
2. **Handshake**: client sends `Hello { protocol_version: 0x03, client_kind: "..." }`.
   - **0x03**: core responds with `Lifecycle(Started)` then `Lifecycle(Ready)`.
   - **0x02** or **0x01** (or any mismatch): core responds `Error { category: "version-mismatch", detail: "bus protocol 0x03 required; received 0x02" }` and closes.
3. **Subscribe / interact**: as slices 001/002. Subscription patterns include the new `family-prefix` value `"buffer/"` for observers of the buffer service's output.
4. **Disconnect**: either side may close the stream; traces record the disconnection. `weaver-buffers` disconnecting retracts every `buffer/*` fact it authored and transitions `watcher/status` to `Stopped`.

## Provenance shape (unchanged from slice 002)

`Provenance` is the per-message attribution record carried on every `Event`, `FactAssert`, `FactRetract`, and every trace entry.

```text
Provenance {
    source: ActorIdentity,              // CBOR tag 1002 — unchanged from slice 002
    timestamp_ns: u64,                  // monotonic nanoseconds since process start
    causal_parent: Option<EventId>      // event that caused this, if any
}
```

**Adjacent-tagged JSON example — fact authored by the buffer service**:

```json
{
  "source": {
    "type": "service",
    "service-id": "weaver-buffers",
    "instance-id": "7b3c5a9e-1234-4abc-9def-111122223333"
  },
  "timestamp_ns": 205436000000,
  "causal_parent": 0
}
```

## Fact families introduced

| Family | Authority | Additive since | Notes |
|---|---|---|---|
| `buffer/path` | `weaver-buffers` | 0.1.0 | `FactValue::String`. Canonical absolute path. Bootstrap fact per buffer entity; never updated. |
| `buffer/byte-size` | `weaver-buffers` | 0.1.0 | **`FactValue::U64`** (new variant). Byte count of in-memory content. |
| `buffer/dirty` | `weaver-buffers` | 0.1.0 | `FactValue::Bool`. Physical dirty: `true` iff memory digest ≠ disk digest. Authority **transferred** from slice 001's `core/dirty-tracking` behavior; wire shape unchanged. |
| `buffer/observable` | `weaver-buffers` | 0.1.0 | `FactValue::Bool`. Per-buffer file observability. `false` during transient failure; edge-triggered. |

`watcher/status` is reused from slice 002 (same `FactValue::String` mirroring `LifecycleSignal`); slice 003 adds `weaver-buffers` as a second authority on the shared fact family, keyed by a distinct instance entity. No authority collision — per-`(family, entity)` authority per slice 002 F10.

## Message-by-message contract (delta from slice 002)

### `Hello` — CHANGED

```
Hello {
    protocol_version: u8,               // 0x03 in this slice (was 0x02)
    client_kind: String
}
```

Version mismatch triggers `Error { category: "version-mismatch", detail: "..." }` followed by close.

### `EventPayload` — CHANGED (remove two, add one)

```
EventPayload = enum {
    // REMOVED from slice 001/002:
    // BufferEdited,
    // BufferCleaned,

    // NEW — slice 003:
    BufferOpen { path: String },
}
```

**Wire shape (adjacent-tagged, kebab-case)**:

```json
{ "type": "buffer-open", "payload": { "path": "/home/alex/file.txt" } }
```

**Semantics**:

- The only producer in slice 003 is the buffer service's own startup (translating each canonicalized positional path to one `BufferOpen` event). TUI- and agent-driven producers arrive in later slices.
- **Idempotence invariant (FR-011a)**: receiving `BufferOpen { path }` whose canonical path corresponds to a buffer entity already owned by the running service instance is a **no-op at the fact level**. No re-bootstrap, no re-read of disk, no fact re-publication. This invariant is wire-level (applies regardless of the producer) and preserves event commutativity per constitution §3.

### `FactValue` — EXTENDED (new variant)

```
FactValue = enum {
    Bool(bool),
    String(String),
    U64(u64),                           // NEW — slice 003
    // ... (additional variants may exist unchanged from slice 001/002)
}
```

**Wire shape** (adjacent-tagged):

```json
{ "type": "u64", "value": 12345 }
```

**Semantics**: unsigned 64-bit integer. Used this slice only by `buffer/byte-size`. Future families may adopt it.

### `FactAssert` / `FactRetract` — unchanged shape; new families publishable

Delivery class remains authoritative. `causal_parent` usage conventions for slice 003:

- **Per-buffer bootstrap**: the four facts (`buffer/path`, `buffer/byte-size`, `buffer/dirty=false`, `buffer/observable=true`) asserted as a single buffer's bootstrap share a **per-buffer** synthesized bootstrap-tick `EventId` as `causal_parent`. Different buffers use distinct `EventId`s.
- **Service-level `watcher/status` transitions**: `causal_parent = None` (lifecycle is the originating signal, per slice 002 convention).
- **Per-buffer `buffer/dirty` and `buffer/observable` transitions**: `causal_parent` carries the poll-tick `EventId` that triggered the transition. Consumers can correlate the retract/assert pair of `buffer/observable` (if applicable) and the re-assert of `buffer/dirty` to the same poll tick.
- **Shutdown retractions**: `causal_parent = None` (the retraction is triggered by SIGTERM/SIGINT, not by an in-graph event).

### `Subscribe` — new pattern value accepted

```
SubscribePattern = enum {
    AllFacts,
    FamilyPrefix(String)
}
```

`FamilyPrefix("buffer/")` subscribes to every `buffer/*` fact (path, byte-size, dirty, observable). `FamilyPrefix("watcher/")` continues to work per slice 002; slice 003 adds `weaver-buffers`-authored instances to that family. `FamilyPrefix("repo/")` continues to work per slice 002.

### Other messages — unchanged shape; updated fact-family vocabulary

`Event`, `InspectRequest`, `InspectResponse`, `StatusRequest`, `StatusResponse`, `Error`, `SubscribeAck` — no shape changes. The inspection render surface (slice 002's `asserting_service` / `asserting_instance` JSON fields) already accommodates `buffer/*` facts authored by `weaver-buffers`; no new fields are introduced.

## Failure modes (P16 alignment)

| Failure | Wire behavior | Trace consequence |
|---|---|---|
| Client sends `Hello.protocol_version = 0x02` (or lower) | Core sends `Error { category: "version-mismatch", ... }`, closes | Trace: connection rejected, version mismatch |
| `weaver-buffers` fails to open a file at startup (missing, permission-denied, directory, oversized) | Service sends a structured `Error { category: "buffer-setup", ... }` diagnostic on stderr (not the bus), retracts any facts already asserted for successful opens, exits non-zero (code 1); no `buffer/*` facts for the failed file are ever asserted | Core sees disconnect; `release_connection` retracts facts the service owned |
| Buffer file unreadable mid-session (permission flicker, file deleted, FS unmount) | Service emits `buffer/observable=false` for that specific entity once on the healthy→unobservable boundary (edge-triggered per slice 002 F21). If this leaves zero observable buffers, additionally emits `watcher/status=degraded`. Subsequent failed polls remain silent. | Trace records per-buffer transition; causal chain preserved via poll-tick `EventId` |
| Buffer file becomes readable again | Service emits `buffer/observable=true` (and `watcher/status=ready` if service had been in `degraded`). | Trace records recovery transition. |
| Two `weaver-buffers` instances on same path | Second instance: handshake succeeds, first `FactAssert` on `buffer/*` for the duplicate entity fails authority check; core sends `Error { category: "authority-conflict", context: "buffer/<path> already claimed by <first-instance-uuid>" }`; second exits code 3 within 1 s | Trace records rejected assertion |
| Buffer service SIGKILL | Bus EOF observed by core; `release_connection` retracts every `buffer/*` and `watcher/status` fact owned by the dropped connection | Trace: retraction sequence authored by `ActorIdentity::Core` (cleanup actor), original assert attribution preserved in the trace entries prior to the retraction |
| Core restart while buffer service connected | Service observes EOF; exits (FR-015 convention from slice 002: auto-reconnect out of scope this slice — revisit later). | No trace (core gone); service process exits cleanly |
| CBOR decode failure on any message | Receiver sends `Error { category: "decode", ... }` and closes | Trace: connection terminated |

## Versioning policy (P7 + P8)

- **Bus protocol** bumps MAJOR `0x02 → 0x03`. `Hello.protocol_version = 0x03` identifies this wire version. `CHANGELOG.md` gains a `## Bus protocol 0.3.0` entry describing the `EventPayload` vocabulary change and the `FactValue::U64` addition.
- **Fact-family schemas** start at 0.1.0 each for every new `buffer/*` family. `CHANGELOG.md` records initial schemas. `buffer/dirty`'s authority-origin transfer (behavior → service) is documented as an annotation under its v0.1.0 entry; the fact's wire shape is unchanged.
- **`watcher/status`** remains at v0.1.0 (slice 002); slice 003 adds a second authority, which is NOT a schema change per arch §5 (authority is conn/entity-keyed; the family itself is shared).
- **CLI + structured output** — see `contracts/cli-surfaces.md`. MAJOR for the `weaver` CLI (removal of `simulate-edit` / `simulate-clean`); additive for `weaver-tui` (new Buffers section); initial 0.1.0 for `weaver-buffers`.

Future compatibility notes:

- Additive `FactValue` variants land as MINOR under CBOR's adjacent-tagged unknown-variant tolerance — but only if subscribers handle unknown `"type"` values gracefully. The core's deserializer currently defaults to `Error { category: "decode", context: "unknown FactValue variant: <X>" }`, which remains a compatibility constraint. Slice 003's variant lands under a MAJOR bump; future variants targeting MINOR bumps may require subscribers to adopt skip-and-log behavior first.
- `EventPayload` additions are additive at the wire but require subscribers to handle unknown values; same constraint.
- `LifecycleSignal` additions (already in slice 002) are additive with the same constraint.

## References

- `specs/003-buffer-service/spec.md` — user stories, functional requirements, success criteria, Clarifications 2026-04-23.
- `specs/003-buffer-service/data-model.md` — full type definitions, fact-family schemas, entity-id derivation, lifecycle state machines.
- `specs/003-buffer-service/research.md` — library choices, observation strategy, CLI error-classification stance.
- `specs/002-git-watcher-actor/contracts/bus-messages.md` — prior wire contract the slice extends.
- `docs/00-constitution.md §17` — multi-actor coherence (actor identity on every message).
- `docs/01-system-model.md §2.4` — Components vs Facts; justification for the `buffer/*` family's derived-fact shape.
- `docs/05-protocols.md §3.4` — actor-identity-and-delegation protocol commitment.
- `docs/07-open-questions.md §18, §26` — undo model and component-infrastructure deferrals.
