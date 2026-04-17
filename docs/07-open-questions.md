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

See system-model §8.

---

## 9. Trace Model — RESOLVED

Resolved: the core maintains an **append-only raw log** of events, fact assertions/retractions, behavior firings, and UI intents, each with causal-parent provenance. Structured trace views (span trees, causal DAGs, timing charts) are UI-side derived views, not a core concern.

See architecture §10.

---

## 10. Prototype Boundary

What is the smallest in-memory prototype that can honestly validate the ontology before introducing distribution?

Partially answered by the current MVP scope (`.agent/mvp.md`). Open question: what's the smallest *next* prototype that introduces real distribution without losing the reflective-loop feel?

---

## 11. Projection vs Source Semantics — RESOLVED

Resolved: **projection-local by default**. Operations that would mutate the source representation must be explicitly routed to the source authority, which may accept or reject them. No implicit write-through.

See system-model §9.1.

---

## 12. Fact-Update Granularity for Non-Quiescent Sources — RESOLVED

Resolved: **event-driven change messages + periodic fact snapshots** by default. Services may declare alternative update models in their lifecycle metadata; consumers adapt to the declaration.

See protocols §3.2, system-model §9.1.

---

## 13. Action-Relevance Through Lossy Projections — RESOLVED

Resolved: **projections carry structured annotations as first-class content** — regions, action targets, semantic markers travel alongside the rendered text. Emacs text-properties promoted to a cross-service protocol. Not a separate out-of-band stream.

See system-model §9.1.

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

## 17. Trace Log Retention and Compaction

The trace log (architecture §10) is append-only; memory grows linearly with session duration. A retention policy is required for sustained use.

Options:

- (a) **Time-based truncation** — entries older than T discarded
- (b) **Size-based truncation** — oldest entries discarded once the log exceeds S
- (c) **Causal-graph pruning** — entries no longer referenced by any live fact, action entity, or derived view are garbage collected
- (d) **Snapshot-and-truncate** — periodic fact-space snapshots allow older entries to be discarded; `why?` walks back to the snapshot rather than to origin
- (e) **Tiered storage** — recent log in memory; older entries paged to persistent storage

Consequences:

- (a)/(b) sever long causal chains; `why?` breaks for anything crossing the horizon
- (c) preserves `why?` by construction but expensive — per-entry reference tracking as facts assert/retract
- (d) bounds memory and keeps `why?` honest up to the current snapshot horizon, which becomes a declared system property
- (e) orthogonal to correctness; addresses scale

Lean: (d) snapshot-and-truncate as the retention model, with `why?` declaring its horizon. (e) as a scale optimization on top when persistence becomes a concern.

---

## 18. Undo Model

No undo model is committed. Without one, users lose work within the first minute of real use.

Shape questions:

- buffer-scoped, session-scoped, or cross-entity?
- can behavior effects (facts asserted in response to edits) be undone, or only buffer content?
- does redo exist?
- how does undo interact with concurrent behavior edits (auto-format firing after a user edit)?

Options:

- (a) **Undo as composed behavior** — a behavior maintains edit history per buffer, reacts to an `undo` action by computing the inverse edit
- (b) **Undo as a core primitive** — the core maintains per-buffer edit history intrinsically, not surfaced through the fact space
- (c) **Undo as a service** — an "undo service" authoritatively owns history facts; undo is a request to that service
- (d) **Undo as versioned content** — the `:content` component tracks versions natively; undo reverts to version N

Consequences:

- (a) ideologically pure and composable; fragile as user-scratch; core cannot guarantee undo works
- (b) reliable and simple; breaks the principle that the core doesn't own behavior logic beyond ontology
- (c) clean authority story; introduces a mandatory service; history-as-facts is expensive if fine-grained
- (d) elegant if content components version anyway; composes well with the component model

Lean: (c) + (d) combined — content components carry lightweight version tags as part of their update model; a governed history service reads them to implement undo/redo. Behavior effects that are pure derivations (e.g., `dirty`) re-derive naturally on revert; effects authored elsewhere become explicit concerns of the history service.

Alternative defensible lean: (a) — if undo-as-composed-behavior compellingly demonstrates the composition model's power. Risk: undo is load-bearing for user trust; fragility is worse than simplicity.

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

## 22. Bus Back-Pressure Beyond MVP

Per-subscriber bounded queue + drop-oldest (architecture §3.1) works for in-memory MVP. At scale with real transports, new concerns surface:

- Network partitions — what happens to queued messages when a subscriber disappears entirely?
- Retracted facts during subscriber absence — does the reconnecting subscriber see the retraction, or only the current state?
- Critical state transitions missed due to drop-oldest — when is "lossy OK" not OK?
- Authoritative replay — does a reconnecting subscriber receive a state snapshot plus deltas, or just a snapshot?

No committed alternatives; resurfaces when the distribution story is concrete (paired with §16).
