# Bus Message Contracts — Slice 002

CBOR-encoded messages on the local Unix-domain-socket bus between `weaver` (core), `weaver-tui`, `weaver-git-watcher`, and any future client. Per L2 P5 / P7 / P8 and `docs/02-architecture.md §3.1`.

**This slice introduces a breaking wire change.** Bus protocol advances **0x01 → 0x02**. The shape of `Provenance.source` changes from an opaque tagged string to a structured `ActorIdentity` (new CBOR tag 1002). `LifecycleSignal` gains three variants. Old clients cannot connect; the handshake rejects mismatched versions with a structured error.

## Naming conventions

Unchanged from slice 001 (Amendment 5). Identifier values on the wire are **kebab-case**; struct field names are `snake_case` in Rust / `camelCase` in JavaScript. Behavior identifiers use `/` as namespace separator; fact attributes are kebab-case with `/`-delimited namespaces (`repo/dirty`, `repo/state/on-branch`, `watcher/status`).

## Wire tagging convention

Unchanged. Adjacent-tagged sum types (`"type"` discriminator + content field).

**New sum type added to the table**:

| Enum             | Content field | Example (JSON)                                                              |
|------------------|---------------|-----------------------------------------------------------------------------|
| `BusMessage`     | `payload`     | `{"type":"fact-assert","payload":{...}}`                                    |
| `SourceId` ❌     | — | **REMOVED** (superseded by `ActorIdentity`)                                                |
| `ActorIdentity` ✅| `id` or variant-specific fields | `{"type":"service","service-id":"git-watcher","instance-id":"<uuid>"}` |
| `SubscribePattern`| `pattern`    | `{"type":"family-prefix","pattern":"repo/"}`                                |
| `FactValue`      | `value`       | `{"type":"bool","value":true}`                                              |
| `LifecycleSignal`| —             | Unit-only; serializes as bare string: `"ready"`, `"degraded"`, etc.         |

## Wire framing

Unchanged. 4-byte big-endian length prefix; one frame per message; 64 KiB max.

## Weaver CBOR tag registry (slice 002)

| Tag number | Meaning | Encoded representation | Status |
|---|---|---|---|
| 1000 | `EntityRef` | CBOR unsigned int | existing (slice 001) |
| 1001 | `Keyword` (slash-namespaced symbol) | CBOR text string | existing (slice 001) |
| **1002** | **`ActorIdentity`** | **CBOR map (adjacent-tagged)** | **added (slice 002)** |

Tag numbers are a public surface per L2 P7. This slice's addition lands with a bus-protocol MAJOR bump per L2 P8.

## Connection lifecycle

Unchanged shape; the handshake carries the new protocol version.

1. **Connect**: client opens a stream to the core's socket.
2. **Handshake**: client sends `Hello { protocol_version: 0x02, client_kind: "..." }`.
   - **0x02**: core responds with `Lifecycle(Started)` then `Lifecycle(Ready)`.
   - **0x01** (or any mismatch): core responds `Error { category: "version-mismatch", detail: "bus protocol 0x02 required; received 0x01" }` and closes.
3. **Subscribe / interact**: as slice 001. Subscription patterns include the new `family-prefix` value `"repo/"` for observers of the watcher's output.
4. **Disconnect**: either side may close the stream; traces record the disconnection. `git-watcher` disconnecting retracts every `repo/*` fact it authored and transitions `watcher/status` to `Stopped`.

## Provenance shape (CHANGED)

`Provenance` is the per-message attribution record carried on every `Event`, `FactAssert`, `FactRetract`, and every trace entry.

```text
Provenance {
    source: ActorIdentity,              // CBOR tag 1002 — see Data Model
    timestamp_ns: u64,                  // monotonic nanoseconds since process start
    causal_parent: Option<EventId>      // event that caused this, if any
}
```

`source` is a structured `ActorIdentity`, not a string. Existing code sites that constructed `SourceId::External("...")` are replaced with `ActorIdentity::Service { service_id, instance_id }` (or the appropriate variant).

**Adjacent-tagged JSON / CBOR example — fact authored by the watcher**:

```json
{
  "source": {
    "type": "service",
    "service-id": "git-watcher",
    "instance-id": "2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90"
  },
  "timestamp_ns": 143522000000,
  "causal_parent": 117
}
```

**Example — fact authored by an in-core behavior (unchanged variant, new enum shape)**:

```json
{
  "source": { "type": "behavior", "id": "core/dirty-tracking" },
  "timestamp_ns": 12340000000,
  "causal_parent": 42
}
```

## Fact families introduced

| Family | Authority | Additive since | Notes |
|---|---|---|---|
| `repo/path` | `git-watcher` | 0.1.0 | Canonical absolute path (bootstrap fact per repo entity) |
| `repo/dirty` | `git-watcher` | 0.1.0 | `bool`. Working tree OR index differs from HEAD; untracked excluded (Clarification Q5) |
| `repo/head-commit` | `git-watcher` | 0.1.0 | SHA-1 (or SHA-256 where repo uses it); text string |
| `repo/state/on-branch` | `git-watcher` | 0.1.0 | Branch name. Mutually exclusive with other `repo/state/*` variants |
| `repo/state/detached` | `git-watcher` | 0.1.0 | Commit SHA. Mutually exclusive |
| `repo/state/unborn` | `git-watcher` | 0.1.0 | Intended branch name. Mutually exclusive |
| `repo/observable` | `git-watcher` | 0.1.0 | `bool`. `false` when the watcher is degraded; retracted on recovery |
| `watcher/status` | `git-watcher` | 0.1.0 | Value is `LifecycleSignal`; keyed by watcher-instance entity |

All new. All authored exclusively by `git-watcher` (single-writer authority per arch §5).

## Message-by-message contract (delta from slice 001)

### `Hello` — CHANGED

```
Hello {
    protocol_version: u8,               // 0x02 in this slice (was 0x01)
    client_kind: String
}
```

Version mismatch triggers `Error { category: "version-mismatch", detail: "..." }` followed by close.

### `Lifecycle` — EXTENDED

```
LifecycleSignal = enum {
    Started,
    Ready,
    Degraded,        // NEW — transient observation failure
    Unavailable,     // NEW — lost observation target or exiting
    Restarting,      // NEW — reserved; not emitted by the core today, emitted by watcher on self-recovery
    Stopped
}
```

Slice 001's core continues to emit only `Started`/`Ready`/`Stopped`. The watcher uses the full vocabulary per `docs/05-protocols.md §5`.

### `FactAssert` / `FactRetract` — unchanged shape; new provenance payload

Delivery class remains authoritative. `causal_parent` usage conventions:

- Initial repo observation publishes (bootstrap): `causal_parent = None`.
- State transitions (`repo/state/*` retract-then-assert pairs): both messages share the same `causal_parent` — typically the watcher's poll-tick event id.
- Watcher lifecycle transitions publish `watcher/status` with `causal_parent = None` (lifecycle is the originating signal).

### `Subscribe` — new pattern value accepted

```
SubscribePattern = enum {
    AllFacts,
    FamilyPrefix(String)
}
```

`FamilyPrefix("repo/")` subscribes to every `repo/*` fact (dirty, head-commit, all `state/*` variants, observable). `FamilyPrefix("watcher/")` observes watcher lifecycle facts. `FamilyPrefix("buffer/")` continues to work per slice 001.

### Other messages — unchanged shape; updated provenance

`Event`, `InspectRequest`, `InspectResponse`, `StatusRequest`, `StatusResponse`, `Error`, `SubscribeAck` carry the new `Provenance.source` shape wherever provenance is part of the payload. No variant additions or removals.

## Failure modes (P16 alignment)

| Failure | Wire behavior | Trace consequence |
|---|---|---|
| Client sends `Hello.protocol_version = 0x01` | Core sends `Error { category: "version-mismatch", ... }`, closes | Trace: connection rejected, version mismatch |
| Watcher fails to open repository (`gix::open` error) | Watcher sends `Error { category: "watcher-setup", ... }`, exits non-zero; no `repo/*` facts asserted | Core sees disconnect; no facts created |
| Watcher loses repository mid-session (permissions flicker) | Watcher emits `Lifecycle(Degraded)` + asserts `repo/observable = false`; existing `repo/state/*` left as-is but observably stale | Trace records degradation; causal chain preserved |
| Watcher loses repository definitively (deleted, unmounted) | Watcher emits `Lifecycle(Unavailable)`, retracts all `repo/*` facts (including `repo/observable`), emits `watcher/status Unavailable`, then `Stopped`, then exits | Trace: full retraction sequence with shared causal parent per retract batch |
| Two watcher instances on same repository | Second instance: handshake succeeds, first `FactAssert` on `repo/*` fails authority check; core sends `Error { category: "authority-conflict", context: "repo/<path> already claimed by <first-instance-uuid>" }`; second exits non-zero | Trace records rejected assertion |
| Core restart while watcher connected | Watcher observes EOF; exits (FR-015 reconnect is out of scope this slice — revisit if needed during implementation) | No trace (core gone); watcher process exits cleanly |
| Behavior firing error in the core (unchanged from slice 001) | `TraceEntry::BehaviorFired { error: Some(...) }` | Trace only |
| CBOR decode failure on any message | Receiver sends `Error { category: "decode", ... }` and closes | Trace: connection terminated |

## Versioning policy (P7 + P8)

- **Bus protocol** bumps MAJOR `0x01 → 0x02`. `Hello.protocol_version = 0x02` identifies this wire version. `CHANGELOG.md` gains a `## Bus protocol 0.2.0` entry describing the provenance-shape change and the `LifecycleSignal` extension.
- **Fact-family schemas** start at 0.1.0 each for every `repo/*` and `watcher/*` family introduced. `CHANGELOG.md` records initial schemas.
- **CLI + structured output** — see `contracts/cli-surfaces.md`. Additive this slice (new binary; `weaver inspect` output grows additive fields).

Future compatibility notes:

- Subsequent additive changes to `ActorIdentity` (new variants) land as MINOR under CBOR's adjacent-tagged unknown-variant tolerance — but only if subscribers handle unknown `"type"` values gracefully. The core's deserializer defaults to `Error { category: "decode", context: "unknown ActorIdentity variant: <X>" }`, which *is* a compatibility constraint; Clarifications and future slices may relax it to "skip-and-log" behaviour.
- `LifecycleSignal` additions are additive at the wire but require subscribers to handle unknown values; same constraint.

## References

- `specs/002-git-watcher-actor/spec.md` — user stories, functional requirements, success criteria.
- `specs/002-git-watcher-actor/data-model.md` — full type definitions.
- `specs/002-git-watcher-actor/research.md` — library and cadence decisions.
- `docs/00-constitution.md §17` — multi-actor coherence (actor identity on every message).
- `docs/01-system-model.md §6` — actor taxonomy materialized by `ActorIdentity`.
- `docs/05-protocols.md §3.4` — actor-identity-and-delegation protocol commitment.
- `docs/07-open-questions.md §25` — shape/migration sub-questions closed by this slice.
- `docs/07-open-questions.md §26` — discriminated-union-facts stopgap tracked here.
