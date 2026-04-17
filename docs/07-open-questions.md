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

## 15. Dynamic Service Discovery

MVP and early iterations use **static service registration** (architecture §2.1). Services are listed in a config file or compiled in.

Open: when and how does Weaver introduce dynamic discovery?

- services advertise themselves over the bus and attach at runtime
- the core tolerates services appearing and disappearing during a session
- authority claims interact with dynamic arrival (can a later-arriving service claim authority over an existing family? Can two instances of the same service compete?)

Becomes relevant alongside distribution and the failure model. Deferred, not rejected.
