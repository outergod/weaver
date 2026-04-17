## Weaver MVP — Definition

### Goal

Prove that Weaver's ontology — **entities, facts, events, behaviors, and applicability** — works end-to-end, over a shared bus, with every meaningful element introspectable, and that a **reflective loop** (live redefinition of behavior without restart) is real.

The MVP is successful if:

- user-visible state and applicable actions are **derived from the fact space**, not hard-coded in the UI
- every fact and action can be explained by walking its provenance
- a composed behavior can be edited, re-evaluated, and observed to change applicability **in the same running session**

---

## Scope

### Core Runtime

* Single-process Weaver Core (no distributed complexity yet)
* Embedded message bus (in-memory or local sockets)
* Typed, namespaced message protocol with at minimum: `event`, `fact-assert`, `fact-retract`, `request`, `response`, `lifecycle`, `error`. Schemas evolve additively; breaking changes require a new namespace.
* **Every message carries provenance**: source, timestamp/sequence, and causal parent where applicable
* Service registration + lifecycle signals (`started`, `ready`, `degraded`, `unavailable`, `stopped`)

### Fact Space (core-owned)

* Opaque entity references (stable IDs, no intrinsic type)
* Assert / retract of facts keyed by `(entity, attribute)`, with provenance metadata attached
* Synchronous query + live subscription over fact patterns
* Authority declaration per fact family (the core rejects authoritative assertions from non-authoritative sources)

### Behavior Engine (minimal)

* At least one wired behavior with the shape: `on <event> when <fact-predicate> → assert/retract/emit`
* Behaviors are authored in **Steel** (see composition-model §7). MVP behaviors are shipped-in-source but loaded through the same path that user-scratch behaviors will use post-MVP.
* Behavior matches are logged with: triggering event, matched facts, produced outputs
* For each event under inspection, behaviors that were evaluated against it but did not fire retain a minimal near-match record (which predicate failed). Retention is scoped to the entity, action, or event being inspected — no global rule-engine trace.
* **Loop-depth guard** is in scope: behaviors firing behaviors beyond a configured depth terminate with the cascade recorded in the trace.
* Behaviors may annotate themselves as **one-shot** or **persistent** (default); one-shot behaviors fire once per matching event and are not re-evaluated until re-armed.

### Composition Runtime (minimal)

* Embedded Steel VM running adjacent to the core, sandbox-bounded by which host primitives are registered
* Host primitives exposed: fact subscription, fact query, event emission, action invocation, user-scratch assertion/retraction. No direct filesystem, network, or process access from Steel.
* **Reflective loop**: the MVP must support reloading a behavior's Steel source in a running session; the new definition takes effect on the next event matching its preconditions, and authoritative fact state is preserved across the reload.

### Applicability

* An applicable action is **materialized as a derived fact** on a stable action entity. The action entity carries at minimum `action/name`, `action/target`, `action/applicable`, and `action/derived-by` (the contributing behavior), all with provenance.
* Applicability changes are fact assertions and retractions on action entities, observable via the same subscription mechanism as any other fact. There is no second object kind for "actions."
* Invoking an action is a request addressed to its action entity; this preserves provenance linkage between the offer of the action and its execution.

#### Action Entity Lifecycle

* **Identity is deterministic**, not minted: an action entity's ID is a function of `(action-type, target)` — e.g., `action:save:<buffer-id>`. Re-derivation across restarts or re-evaluation yields the same ID. Minted IDs would break `why?` across sessions and sever causal links.
* **Existence is bounded by the target's lifetime, not by applicability.** The action entity materializes when its target exists and a behavior defining the action is registered; it ceases when the target ceases. `action/applicable` toggles within that window. A closed buffer's save-action entity is gone, not dormant.
* **The defining behavior authors both existence and applicability** in one declaration: "for every entity matching predicate P, an action entity exists; `action/applicable` holds when condition C." Existence-fact and applicability-fact share provenance.
* **Targets generalize to target tuples** for multi-entity actions (e.g., `compare($b1, $b2)`). Untargeted actions bind to a system-scope entity so `(action-type × target)` stays uniform. MVP only requires the single-target case.
* Clients may subscribe to `action/applicable = true for target E` (leader-menu view) or to all action entities for `E` (greyed-out view with reasons). The ontology enables both; neither is privileged.

### Introspection

* The core maintains an **append-only trace log** of events, fact assertions/retractions, behavior firings, and UI intents, each with causal-parent provenance. This is the backing store for `why?`.
* A `why?` request that, given an entity or an applicable action, returns:

  * the facts currently holding on it
  * the source/authority of each fact
  * the behaviors that contributed
  * the causal chain back to the originating event (walked from the trace log)
* The trace log is subscribable; a second client can observe history as it accretes without special privileges.
* This channel is available on the same bus as everything else.

---

### Authorities and Services (minimum set)

#### Core (authoritative)

* Buffer entities and buffer-open state (per architecture §5)
* Dirty / clean state of buffers
* Entity registry and the fact space itself

#### Filesystem Service (authoritative for path facts)

* Publishes facts about observed paths (existence, mtime) with provenance
* Responds to read and write requests
* Emits events on external change (`fs/changed`)
* Fact assertions, not opaque notifications: change is represented as updated facts + an event

#### Action Execution (cross-authority coordination)

Actions whose consequences span authorities are **orchestrated by the core**, not by services. This rule holds regardless of which authorities the action touches — including the case where the core is authoritative over *none* of the affected fact families. See architecture §11.

* Core owns the **semantics** of the action: applicability derivation, state transitions, cross-fact invariants.
* Services own the **path-state consequences** within their own fact families and perform the concrete work.
* `save` is the canonical case for MVP. The `save` action entity and its applicability live in the core, derived from `(dirty $buffer)`. Invocation lands at the core, which issues a write request to the filesystem service; on its successful response the core retracts `(dirty $buffer)` and the filesystem service updates its path facts. The filesystem write and the fact retraction are independent, traceable events, causally linked through provenance — neither masquerades as the other.
* Services must not expose shortcuts that let clients bypass core orchestration for actions the core owns semantically.

---

### Interaction Layer (required)

A minimal **TUI** that:

* Connects to the core via the **same bus interface** as any service
* **Derives all visible state from fact subscriptions** — it holds no privileged knowledge of service APIs
* Renders:

  * entities and facts currently holding on them
  * the event stream
  * the set of **currently applicable actions** (derived from fact + behavior state, not hard-coded menus)
  * a **command-vocabulary view** — the action-entity space queryable by name or target (the `M-x` analog; see interaction-model §11)
  * service lifecycle status
* Allows:

  * issuing requests that cause events and fact changes
  * invoking the `why?` channel on any visible entity or action
  * navigating between views (primitive is fine)
  * triggering a reload of the behavior source file (the reflective-loop surface)

The TUI must distinguish **shared semantic state** (subscribed facts) from **client-local view state** (layout, cursor, scroll). The latter never leaves the client.

No persistence of layout, no theming, no polish.

---

## Required Workflow (the vertical slice)

A user must be able to:

1. Browse paths surfaced as filesystem facts
2. Open a file, producing a buffer **entity** and core-asserted facts (e.g., `buffer/path`, `buffer/open`)
3. Edit the buffer; a behavior asserts `(dirty $buffer)` in response to the edit event
4. Observe that the **save** action becomes applicable *because* `(dirty $buffer)` holds — not because a menu item is wired
5. Invoke save on the action entity. Core orchestrates: it requests a write from the filesystem service, and on its success retracts `(dirty $buffer)`. The filesystem write and the fact retraction are independent, traceable events, causally linked through provenance
6. Invoke `why?` on the buffer and on the save action and see the fact/behavior chain that produced them
7. Edit the Steel source of the dirty-tracking behavior, trigger reload, and observe the change take effect on the next edit — without restarting the core and without losing the open buffer's authoritative state

---

## Constraints

* No network/distributed concerns yet (everything local)
* No plugin system beyond hardcoded services
* Composition language is Steel, but MVP behaviors are shipped in source — no runtime-authored user-scratch behaviors yet (the lane exists architecturally; exercising it is post-MVP)
* No authentication/security layer
* No complex diffing or CRDTs (full-text replace is fine)
* No web UI

---

## Non-Goals

* Performance optimization (but the three latency classes — immediate / interactive / asynchronous — are *named*, and each request carries its declared class even if not enforced to spec)
* Scalability
* Stable public APIs
* Final UI/UX decisions
* Runtime-authored user-scratch behaviors (the architectural lane exists; MVP only exercises shipped behaviors via the same load path)
* Service scaffolding tooling (the eventual "defun-cheap service" — post-MVP)
* Shell execution, task runners, or any service not required by the vertical slice

---

## Acceptance Criteria

* Services and the TUI communicate exclusively via the bus (no hidden direct calls)
* Every event, fact, and response carries provenance metadata
* **Fact assertion / retraction is a distinct message category from events** (not collapsed into notifications)
* Authority over each fact family is declared and enforced
* At least one behavior is exercised end-to-end, and its firing is inspectable (inputs, match, outputs)
* Behaviors are authored in Steel and loaded through the composition runtime
* **The reflective loop works**: editing a behavior's source and triggering reload changes applicability on the next matching event, without restart and without losing authoritative fact state
* **The TUI derives visible state and applicability from fact subscriptions**, not from local knowledge of service APIs
* The command-vocabulary view lists action entities derived from the fact space — the same query that powers contextual menus
* The `why?` channel answers, for any visible entity or applicable action, which facts and which behaviors produced it
* Every request schema declares its latency class (immediate / interactive / asynchronous), even if not enforced
* The full workflow (open → edit → dirty → save → clean → reload behavior → observe change) works reliably
* If the filesystem service is killed, the core stays responsive; dependent facts are marked stale or retracted with a recorded reason

---

## Failure Conditions

The MVP is considered a failure if:

* The TUI knows about buffers or files as anything other than entities with facts
* Applicability is decided in the TUI instead of derived from the fact space
* Events and fact changes are indistinguishable on the wire
* An action is available but the system cannot explain why
* Adding a second consumer of the fact stream (e.g., a logging view) requires changes to the services
* A service crash causes the core to hang or silently corrupt state
* Redefining a behavior requires restarting the core, or loses authoritative state across the edit
* Behaviors are callable from the TUI through a code path that does not also exist for any other client

---

## Guiding Heuristics

If the TUI can be replaced by a different client that subscribes to the same facts and invokes the same requests, and the user's workflow still works, the ontology is honest.

If the TUI needs private knowledge about services to render state or surface actions, the ontology is leaking.

If a behavior edit requires a restart to take effect, the reflective loop is not real and the vision is not preserved — even if everything else works.
