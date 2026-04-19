# Protocols

This document describes the conceptual protocol requirements for communication between core and services.

## 1. Message Categories

Weaver communication consists of messages in at least these categories. Each category has a default **delivery class** (architecture §3.1) that governs sequence guarantees, gap detection, and reconnect/replay behavior:

| Category | Default delivery class |
|---|---|
| `event` | lossy |
| `fact-assert` | authoritative |
| `fact-retract` | authoritative |
| `request` | request/response correlation; loss detected by requester timeout |
| `response` | paired with request |
| `stream-item` | lossy (loss is acceptable within a stream; stream boundaries are authoritative) |
| `lifecycle` | authoritative |
| `error` | authoritative |

Authoritative messages carry per-publisher monotonic sequence numbers; subscribers detect gaps and may request snapshot-plus-deltas on reconnect. Lossy messages may be dropped under back-pressure (per-subscriber `drop-oldest` queue). See architecture §3.1 for the full delivery contract and §3.2 for permissible subscriber overrides.

---

## 2. Events

An event message represents occurrence or intent.

An event should include:

- event name
- payload
- source
- causal parent if any
- timestamp or sequence identifier

Events are transient and may trigger behavior.

### 2.1 Self-Caused Events

An authority **must not** emit an event for a state transition that is causally attributable to its own just-handled request, unless the event semantically represents something distinct from the request's effect.

Example: the filesystem service handling a `write` request must not emit `fs/changed` for the file it just wrote on behalf of that request. Doing so causes clients (or the core) to react as if an external change occurred, producing silent reactive loops and incorrect buffer reloads.

Enforcement is service-side discipline — the protocol does not police it. The implementation pattern is causal-parent correlation: services track their in-flight request handlers and suppress self-caused emissions within that scope. Weaver commits to providing reusable abstractions (correlation tokens, request-scope guards, service-framework helpers) in a later iteration so that service authors get this right by default rather than by attention.

Until those abstractions exist, service authors bear the discipline directly, and violations are detectable in traces (a causal chain that loops back to the authority that started it).

---

## 3. Facts

A fact assertion should include:

- entity reference
- attribute or relation name
- value
- source
- authority status
- derivation metadata if applicable
- freshness metadata

A fact retraction should identify the fact being withdrawn and why.

### 3.1 User-Scratch Provenance

Facts asserted from the composition runtime without governed authority must carry `source: user-scratch:<origin>` in their provenance (see system-model §2.3, composition-model §10.2). Consumers may filter or weight by authority.

### 3.2 Update Granularity for Non-Quiescent Sources

Services authoritative over fact families that mutate continuously (live DOM under JS, streaming logs, process output, files under active write) emit updates as **structured change events with periodic fact snapshots** by default.

Services may declare an alternative update model in their lifecycle metadata — coalesced snapshots only, change events without snapshots, or a bespoke cadence. Consumers read this declaration and adapt their subscription pattern accordingly.

This keeps the fact space tractable under high-rate sources while preserving service autonomy over their own update model.

### 3.3 Hosted Origin

Assertions emitted by a **language host** on behalf of user code it runs carry a `hosted-origin` subfield in their provenance, in addition to the authoritative `source` (the host itself).

`hosted-origin` identifies:

- hosted file (or equivalent identifier)
- hosted location (line, range, function name where meaningful)
- hosted-runtime version (e.g., `python-3.12.4`, `node-22.1.0`)

The host remains the authoritative `source` — authority does not fragment across hosted users. See architecture §9.1.1.

---

## 4. Requests and Responses

Requests are explicit asks for work or information.

Responses may be:

- immediate
- deferred
- streaming
- partial
- final

All nontrivial requests should be cancellable.

### 4.1 Latency Commitments

Each request schema declares the latency class of its response (see architecture §7.1: immediate / interactive / asynchronous). Breaches are observable in traces.

---

## 5. Lifecycle Messages

Services must be able to communicate lifecycle state such as:

- started
- ready
- degraded
- unavailable
- restarting
- stopped

This information must be available to interaction and tracing layers.

---

## 6. Error Requirements

Errors must be structured.

They should include:

- source
- category
- message
- affected request or event if any
- retryability if known

Silent failure is not acceptable protocol behavior.

---

## 7. Schema Requirements

Names, payloads, and semantics are schema-governed.

The protocol commits to:

- **namespacing** — every event, fact family, and request type lives in a named namespace
- **additive-only evolution** — fields may be added with explicit optionality; existing fields may not be removed or have their semantics changed
- **breaking changes via namespace migration** — incompatible changes require a new namespace (`git.v1/…` → `git.v2/…`), not an in-place rewrite
- **explicit optionality** on every added field

Typed facts and typed events are desirable.
Untyped entities remain acceptable and preferable.
