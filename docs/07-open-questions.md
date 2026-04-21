# Open Questions

This document tracks unresolved questions and productive tensions.

## 1. Composition Language — RESOLVED

Resolved: **Steel**, with the extension model as two lanes (services for governed capabilities, Steel for composed behaviors and user-scratch facts).

See composition-model §7 for the choice, commitments, and caveats; constitution §14 for the two-lane principle.

---

## 2. Fact Semantics — RESOLVED

Resolved: flat tuples `(entity, attribute, value, provenance)`. Values may be entity references (relation-valued facts), primitives, or small structured values. Freshness is a provenance field, not a separate fact kind. Richer nested records are deferred.

See system-model §2.

Temporal versioning beyond provenance timestamps remains open.

---

## 3. Tags Versus Predicates — RESOLVED

Resolved: tags are pure convenience over `(entity, tag, true)` facts — no privileged handling. Predicates are the primary evaluation mechanism.

Performance commitment: behavior preconditions drive **lazy, incremental, shared indexes** keyed by predicate shape. Inspired by archetype-based ECS (Bevy, Flecs). See architecture §4.1.

---

## 4. Derived Views — RESOLVED (for MVP)

Resolved for MVP and early iterations: **on-demand recomputation**. Predicate-shape indexing (architecture §4.1) handles behavior evaluation's hot path; other derived views compute at query time.

Incremental maintenance, caching with invalidation, and partial delegation to services remain available post-MVP optimizations, to be adopted per-view-kind when measurement demonstrates need.

See architecture §4.2.

---

## 5. Event Loops and Stability — PARTIALLY RESOLVED

Resolved for MVP: **causality tracking + loop-depth guard** (a), and **explicit one-shot vs persistent behaviors** (d). See composition-model §12.1–12.2.

Remains open: **idempotence contracts** (b) and **transactional boundaries around behavior batches** (c). They become relevant if the committed mechanisms prove insufficient. See composition-model §12.3.

---

## 6. Authority Boundaries — RESOLVED

Resolved: single authority per canonical fact family; competing authoritative claims are rejected. Derived, speculative, and user-scratch facts may coexist alongside authoritative ones but must be marked accordingly.

Entity lifetime is the responsibility of the authority owning the entity's primary fact family; retraction cascades to dependent action entities and derived facts.

See architecture §5 and §5.1, system-model §2.3.

---

## 7. Workspace Semantics — RESOLVED

Resolved: a workspace is itself a fact that participates in applicability predicates like any other. No privileged containment semantics. This preserves constitution §8 (workspaces as lenses, not containers).

See constitution §8, interaction-model.

---

## 8. UI Intent Model — RESOLVED

Resolved: UI intents are structured records `(intent-type, target-entity, parameters, source-behavior)`. Clients are free to ignore them. `source-behavior` integrates intents into the trace model, avoiding a second provenance mechanism.

See system-model §9.

---

## 9. Trace Model — RESOLVED

Resolved: the core maintains an **append-only raw log** of events, fact assertions/retractions, behavior firings, and UI intents, each with causal-parent provenance. Structured trace views (span trees, causal DAGs, timing charts) are UI-side derived views, not a core concern.

See architecture §10.

---

## 10. Prototype Boundary

What is the smallest in-memory prototype that can honestly validate the ontology before introducing distribution?

Partially answered by the current Ontology Prototype scope ([`mvp-ontology.md`](mvp-ontology.md)) and extended by the Editor MVP scope ([`mvp-editor-projection.md`](mvp-editor-projection.md)). Open question: what's the smallest *next* prototype that introduces real distribution without losing the reflective-loop feel?

---

## 11. Projection vs Source Semantics — RESOLVED

Resolved: **projection-local by default**. Operations that would mutate the source representation must be explicitly routed to the source authority, which may accept or reject them. No implicit write-through.

See system-model §10.1.

---

## 12. Fact-Update Granularity for Non-Quiescent Sources — RESOLVED

Resolved: **event-driven change messages + periodic fact snapshots** by default. Services may declare alternative update models in their lifecycle metadata; consumers adapt to the declaration.

See protocols §3.2, system-model §10.1.

---

## 13. Action-Relevance Through Lossy Projections — RESOLVED

Resolved: **projections carry structured annotations as first-class content** — regions, action targets, semantic markers travel alongside the rendered text. Emacs text-properties promoted to a cross-service protocol. Not a separate out-of-band stream.

See system-model §10.1.

---

## 14. Behavior-Reload Semantics for Behavior-Authored State — RESOLVED

Resolved: **strict re-derivation by default**. On reload, user-scratch facts asserted by the prior version are retracted; user-scratch families declared by the prior version are dropped unless re-declared; action-entity applicability re-evaluates against the new version on the next relevant event. Authors opt into per-family preservation by marking families as `:reload-preserve`.

See composition-model §11.1.

---

## 15. Service-Author Abstractions for Self-Cause Discipline

The protocol (§2.1) requires authorities to suppress events causally attributable to their own just-handled requests. Enforcement is currently service-side discipline.

Follow-up: provide first-class abstractions for this — correlation tokens, request-scope guards, service-framework helpers that suppress self-caused emissions by default. Service authors should get this right by construction, not by vigilance.

Shape of the abstractions, how they compose with async request handlers, and whether they live in the bus SDK layer or in a higher-level framework — all open.

---

## 16. Dynamic Service Discovery

MVP and early iterations use **static service registration** (architecture §2.1). Services are listed in a config file or compiled in.

Open: when and how does Weaver introduce dynamic discovery?

- services advertise themselves over the bus and attach at runtime
- the core tolerates services appearing and disappearing during a session
- authority claims interact with dynamic arrival (can a later-arriving service claim authority over an existing family? Can two instances of the same service compete?)

Becomes relevant alongside distribution and the failure model. Deferred, not rejected.

---

## 17. Trace Log Retention and Compaction — RESOLVED

Resolved in architecture §10.2: snapshot-and-truncate is the committed retention model, with the snapshot horizon as a declared system property queryable via `why?`. Tiered storage (option e) remains an orthogonal scale optimization. Time-based truncation (a/b) and causal-graph pruning (c) are explicitly rejected.

Original options preserved for context:

- (a) **Time-based truncation** — entries older than T discarded
- (b) **Size-based truncation** — oldest entries discarded once the log exceeds S
- (c) **Causal-graph pruning** — entries no longer referenced by any live fact, action entity, or derived view are garbage collected
- (d) **Snapshot-and-truncate** — periodic fact-space snapshots allow older entries to be discarded; `why?` walks back to the snapshot rather than to origin **[ADOPTED]**
- (e) **Tiered storage** — recent log in memory; older entries paged to persistent storage **[ADOPTED as orthogonal optimization]**

---

## 18. Undo Model — RESOLVED for Editor MVP

The undo model is committed as a gate of the Editor MVP ([`mvp-editor-projection.md`](mvp-editor-projection.md) Gate 1). The Ontology Prototype ([`mvp-ontology.md`](mvp-ontology.md)) does not include undo and is honestly named to reflect that. Editor-shaped use waits for the Editor MVP.

**Committed shape: (c) + (d) combined.** Content components carry lightweight version tags as part of their update model; a governed history service reads them to implement undo/redo. Behavior effects that are pure derivations (e.g., `dirty`) re-derive naturally on revert; effects authored elsewhere become explicit concerns of the history service.

Original options preserved for context:

- (a) **Undo as composed behavior** — a behavior maintains edit history per buffer, reacts to an `undo` action by computing the inverse edit
- (b) **Undo as a core primitive** — the core maintains per-buffer edit history intrinsically, not surfaced through the fact space
- (c) **Undo as a service** — an "undo service" authoritatively owns history facts; undo is a request to that service **[ADOPTED]**
- (d) **Undo as versioned content** — the `:content` component tracks versions natively; undo reverts to version N **[ADOPTED, paired with (c)]**

Rejection rationale:

- (a) — too fragile for user trust; undo is load-bearing.
- (b) — breaks the principle that the core does not own behavior logic beyond ontology.

Shape questions remain partially open and resolve during Editor MVP implementation:

- buffer-scoped, session-scoped, or cross-entity? — answered case-by-case in the history service's design.
- redo? — yes, symmetric to undo.
- undo and concurrent behavior edits (auto-format firing after a user edit)? — Editor MVP Gate 1 acceptance requires explicit semantics; expected resolution is "the history service records both, and undo of the user edit reverts both atomically."

---

## 19. Cursor, Selection, Point/Mark as Shared vs Local

Cursor and selection are client-local view state by default (constitution §11). But many useful behaviors depend on cursor position (completion triggers, contextual actions). When does cursor promote to shared?

Options:

- (a) **Always local** — cursor never enters the fact space; cursor-dependent behaviors are impossible
- (b) **Local by default, shared on explicit opt-in** — the client declares "my cursor is shared" as a fact; collaboration, cross-client visibility, and cursor-aware behaviors subscribe
- (c) **Shared by construction** — cursor is always a fact; single-client is a degenerate case

Consequences:

- (a) simplest; forecloses a large class of useful behaviors
- (b) clean principle; requires a promotion API; most real systems need this eventually
- (c) maximum flexibility; every cursor move is fact churn at keystroke rate

Lean: (b) local by default, opt-in for sharing. Cursor-aware behaviors subscribe to whatever cursor facts are published when the client opts in. Keeps single-client cursor fast; enables cursor-aware composition when requested.

---

## 20. Content-Addressable Projections for Large Buffers

With `:content` as a component (system-model §2.4), fetching whole content for every query is inefficient when behaviors want "just this range" or "just this symbol."

Options:

- (a) **Range fetches** — component query accepts a range, returns just that range
- (b) **Named projections** — components declare named projections (`:symbol-at-point`, `:first-line`, `:function-containing-point`); behaviors request by name
- (c) **Virtual sub-entities** — each meaningful projection materializes as its own entity with facts
- (d) **Range subscriptions** — behaviors subscribe to a content range; receive updates only when that range changes

Consequences:

- (a) simplest; inefficient for repeated same-range queries; no incrementality
- (b) cleaner composition; fixed vocabulary; must be declared ahead
- (c) uniform (everything is an entity) but massively multiplies entity count
- (d) efficient for continuous observation; complex to implement

Lean: (a) + (d) — range fetches as the basic primitive, range subscriptions as the optimization for repeat observers. (b) as sugar over (a) once patterns emerge. Avoid (c); projection-as-entity fragments the ontology.

---

## 21. Ephemeral / In-Memory-Only Buffers

Path-less buffers (scratch, draft, compilation output, REPL) exist conceptually; MVP begins with "browse files" and excludes them (non-goal).

Policy (largely implied by existing architecture; no real tension):

- Path-less buffers are first-class buffer entities; no filesystem facts until a path is assigned
- `save` action's applicability requires a path; when absent, `save` is not applicable
- A `save-as` action takes a path argument; it is applicable to any unsaved buffer and asserts `buffer/path` as part of its execution
- Closing an unpathed buffer is entity retraction; content is lost unless the user saved-as first

Applied directly when path-less buffers land post-MVP. No further design tension.

---

## 22. Bus Back-Pressure Beyond MVP — RESOLVED

Resolved in architecture §3.1 (Delivery Classes): authoritative messages (`fact-assert`, `fact-retract`, `lifecycle`, `error`) carry per-publisher monotonic sequence numbers; subscribers detect gaps and receive snapshot-plus-deltas on reconnect. Lossy messages (`event`, `stream-item`) keep `drop-oldest` semantics. `block-with-timeout` (bounded, never `block-forever`) is the default back-pressure for authoritative messages; lossy messages drop oldest under back-pressure.

The original concerns map as follows:

- **Network partitions / subscriber disappearance** — authoritative class buffers within transport limits; on reconnect, snapshot-plus-deltas brings the subscriber current.
- **Retractions missed during absence** — captured in the per-fact-family snapshot taken at reconnect; retractions that occurred between snapshot and reconnect arrive in the delta stream.
- **Critical state transitions missed due to drop-oldest** — these messages are now in the authoritative class and are not subject to drop-oldest at the wire level.
- **Snapshot vs. snapshot+deltas on reconnect** — committed to snapshot+deltas; per-publisher sequence numbers make it implementable.

Distribution-specific refinements (cross-network sequence ordering, snapshot transfer cost) become relevant when the distribution story is concrete and pair with §16.

---

## 23. Authorship vs Provenance

Constitution §17 requires provenance to carry the originating actor. Protocols §3.4 introduces `on-behalf-of` as an optional delegation subfield. Open: how far does the delegation chain go, and how is it validated?

- shape of the delegation chain — single delegator, nested chain, or a set of co-authorizers?
- validation — does the delegator's identity have to be signed, or is claim-based attribution sufficient for MVP?
- UI presentation — when the user refuses an agent contribution (§17 reversibility), does the refusal retract along the delegation chain or only at the primary `source`?

Deferred until a slice requires delegation semantics beyond the existing hosted-origin pattern (candidate: agent-integration follow-up slice).

---

## 24. Speculative-Fact Mechanism

Constitution §11 (convergence clause) allows temporary divergence with reconcilable shared state. Speculative or provisional contributions — an agent proposing a change the user has not yet accepted, a behavior computing a what-if — have no declared mechanism today.

Options:

- (a) **Separate fact-space partition** — speculative facts live in a staging space outside the authoritative fact space; promotion to authoritative is explicit
- (b) **Provenance flag** — speculative facts live in the normal fact space with a `speculative: true` provenance field; authority rules define visibility
- (c) **Per-actor shadow** — each actor maintains a speculative overlay visible to itself; promotion requires the governing authority to accept

Consequences:

- (a) clean separation, harder integration with applicability rules
- (b) minimal mechanism, risk of silent coalescence into authoritative state
- (c) strong isolation, complex multi-actor coordination

Deferred until a slice demonstrably needs speculative contributions. The constitution commits only to "reconcilable" shared state (§11); it does not promise the mechanism exists.

---

## 25. SourceId Evolution to Typed ActorKind — PARTIALLY RESOLVED

The core previously carried `SourceId::{Core, Behavior(id), Tui, External(String)}` in provenance. The `External(String)` variant was an opaque placeholder; it collapsed all out-of-process actors into a single unstructured tag.

Constitution §17 and system-model §6 name five actor kinds (users, services, embedded behaviors, language hosts, external agents). A structured `ActorKind` is the natural code-level expression of this taxonomy.

Resolved by slice 002 (`specs/002-git-watcher-actor/` Clarifications Q1 and Q2):

- **shape** — single closed enum, one variant per actor kind, payload per variant. Matches the §6 taxonomy; future kinds are added as new variants under additive-evolution rules (L2 Principle 15).
- **migration** — `SourceId::External(String)` is replaced entirely; no parallel support, no deprecation shim. Breaking at the wire; paired with a bus protocol version bump (L2 Principle 8).

Remains open:

- **identity stability across sessions** — how is an actor's identity persisted so `on-behalf-of` chains in stored traces survive restarts, upgrades, and re-deployments? Slice 002 generates watcher instance identities as random UUIDs per invocation (spec Clarification Q3), deliberately *not* persisting identity across restarts. A stable identity scheme becomes relevant when delegation chains must survive trace-horizon boundaries — candidate trigger is the agent-delegation slice.

---

## 26. Discriminated-Union Facts: Naming-Based Stopgap vs. Components

Some fact families naturally express discriminated unions — one unit-concept with mutually exclusive variants. Working-copy state (`on-branch` / `detached` / `unborn` / `rebasing` / `merging` / …) is the motivating instance. Weaver's fact-value type is primitive-only today (`FactValue::{Bool, String, Int, Null}`; system-model §2 explicitly defers richer nested records). A discriminated union therefore cannot cross the wire as one typed fact value under the current regime.

Slice 002 (`specs/002-git-watcher-actor/`, Clarification Q4) adopts the stopgap: **discriminated-union-by-naming** under a shared family (`repo/state/*`), with the authoring service enforcing the mutex invariant (exactly one variant asserted per entity at any time). This lets the slice ship without extending `FactValue` or introducing component infrastructure.

**Known costs of the stopgap:**

- Mutex invariant lives in producer code, not the type system — a producer bug can admit inconsistent state that consumers silently observe.
- State transitions appear in the trace as retract-then-assert pairs; consumers must pair them cognitively.
- "Subscribe to any state change" fragments across N predicate-shape indexes (architecture §4.1).
- Schema discoverability: union membership is implicit in the naming convention only.

**Candidate long-term resolutions:**

- (a) **Extend `FactValue` with a `Variant` or `Record` case.** A discriminated union becomes one typed value on one attribute. Requires wire-format change, CBOR tag addition, serializer/deserializer updates. Risk: pressure to stuff arbitrary structured data into `FactValue` erodes the fact/component boundary (system-model §2.4).
- (b) **Lean into components** (system-model §2.4). A discriminated union becomes a typed *component* attached to an entity; the authority updates it in place and emits derived facts for behaviors to predicate on (`repo/on-branch ?name`, `repo/detached`, …). §2.4-native. Requires component infrastructure in code — a `Component` type, point-query primitive, update-in-place semantics — none of which exist today.
- (c) **Accept the naming-based stopgap permanently** and document it as the chosen idiom. Producers carry the mutex invariant as a first-class responsibility. No architectural work required; cost is ongoing and cumulative.

**Revisit triggers (any one should prompt reconsideration):**

1. A *third* producer publishes discriminated-union-shaped facts and begins reinventing the mutex pattern across services.
2. Behaviors routinely subscribe to the full family rather than specific variants, indicating the naming split works against the consumption pattern.
3. Traces become difficult to read because retract-then-assert pairs dominate the transition events under analysis.

**Current lean:** **(b) — components.** §2.4 already commits to the fact/component distinction; slice 002 is an acknowledged stopgap, not a rejection. Deferred until at least one revisit trigger fires.

First concrete instance: `specs/002-git-watcher-actor/` Clarification Q4 (`repo/state/*`).
