# System Model

Weaver models the world in terms of entities, facts, events, behaviors, and authorities.

## 1. Entities

An entity is an opaque, stable, addressable reference.

Entities do not have intrinsic type, class, or method ownership.

Examples of things that may be represented by entities include:

- an open buffer
- a file path
- a project root
- a workspace
- a symbol occurrence
- a git repository
- a search session
- a diff hunk
- a task run

An entity is only an anchor for facts and relations.

---

## 2. Facts

Facts are assertions about entities.

Examples:

- an entity has a path
- an entity is open in a workspace
- an entity belongs to a project
- an entity has diagnostics
- an entity is associated with a git repository

Facts may be:

- asserted
- retracted
- updated
- authoritative
- derived

Facts must carry provenance metadata, including at minimum:

- source
- authority status
- timestamp or version
- freshness (staleness is a field in provenance, not a separate fact kind)
- derivation information if applicable

Facts should be small, explicit, and composable.

Fact shape is a flat tuple: `(entity, attribute, value, provenance)`. Values may be entity references (relation-valued facts), primitives, or small structured values. Richer nested records are deferred.

### 2.1 Tags

Tags are lightweight facts used as shorthand.

Examples:

- `:buffer`
- `:dirty`
- `:visible`
- `:git-repo`

Tags are conveniences, not essences.

They do not confer privileged ontology.

### 2.2 Fact Families

Some facts naturally cluster into semantic families, such as:

- buffer-related facts
- project-related facts
- workspace-related facts
- git-related facts

These families organize meaning, but do not define intrinsic entity type.

### 2.3 User-Scratch Facts

Facts asserted by composed behaviors that have not been promoted into a governed family are tagged `source: user-scratch:<origin>` in their provenance.

User-scratch facts:

- are observable alongside governed facts
- may participate in applicability predicates
- never claim authority over a fact family
- never shadow a governed authority's claims
- are session- or behavior-scoped by default; persistence is explicit

Behaviors that predicate on user-scratch facts must be legible as such in traces — the `why?` channel surfaces non-authoritative provenance explicitly.

---

## 3. Events

Events represent change, occurrence, or intent.

Examples:

- a buffer was opened
- a buffer was saved
- a workspace was activated
- a search was requested
- a service became unavailable
- a compare action was requested

Events are transient.

They may trigger behaviors, but they are not themselves persistent world state.

Events must carry provenance and causal metadata where possible.

---

## 4. Behaviors

Behaviors are reactive units that respond to events and fact patterns.

A behavior may:

- assert facts
- retract facts
- emit events
- propose actions
- produce UI intents
- request external work

Behaviors do not own entities.

Behaviors become applicable when their preconditions match current context.

### 4.1 Preconditions

Behavior applicability is determined by:

- event type
- fact predicates
- service availability
- contextual constraints

Behaviors match contexts, not classes.

### 4.2 Effects

Behavior results must be explicit and traceable.

A behavior should never be semantically invisible.

---

## 5. Authorities

Some services are authoritative over specific fact families.

Examples:

- editor core may be authoritative for open-buffer state
- project service may be authoritative for project membership facts
- git service may be authoritative for repository state
- language service may be authoritative for diagnostics

Authority constrains who may publish canonical facts in a given domain.

Derived facts may exist alongside authoritative facts, but must say so explicitly.

---

## 6. Derived Views

A derived view is an interpretation assembled from underlying facts.

Examples:

- current entity context
- available actions for a leader menu
- visible entities in the active workspace
- compareable buffers
- entities relevant to a search result

Derived views are not privileged ontology.

They are projections over the fact space.

---

## 7. Applicability

Actions are not owned by entities.

An action is applicable when the current combination of facts, events, and available services satisfies the relevant behavior or action rule.

This is the central mechanism by which capability emerges in Weaver.

### 7.1 Action Entities

Applicability is materialized as derived facts on stable **action entities**.

An action entity's identity is deterministic — a function of `(action-type, target)`, generalized to `(action-type, target-tuple)` for multi-target actions. Re-derivation across sessions yields the same ID.

Existence is bounded by the target's lifetime, not by applicability. The action entity materializes when its target exists and a behavior defining the action is registered; it ceases when the target ceases. The `action/applicable` fact toggles within that window.

Untargeted actions (session-level commands) bind to a system-scope entity so `(action-type × target)` remains uniform.

The set of all action entities constitutes Weaver's **command vocabulary** — the canonical namespace for "what can be done in this system." Command discovery is a query over this space, optionally filtered by target or by applicability.

---

## 8. UI Intents

UI intents are structured records emitted by behaviors.

Shape:

- `intent-type` — the named intent (`highlight`, `focus`, `display-result-set`, `reveal-relation`, …)
- `target-entity` — the entity the intent applies to (or a target tuple)
- `parameters` — intent-specific payload
- `source-behavior` — the behavior that emitted the intent, for trace integration

Intents:

- reference entities and facts
- describe desired presentation effects
- are not authoritative — clients are free to interpret or ignore them
- are observable in traces via `source-behavior`

Interpretation is left to clients.

---

## 9. Substrate

A substrate is an interface — a set of fact predicates — that any entity may satisfy to participate in a class of uniform operations.

The primary substrate is the **buffer substrate**: any entity satisfying buffer-family predicates (content-available, viewable, navigable) participates in buffer operations (search, navigation, kill/yank, compare, annotate) regardless of its underlying representation.

A text file, a web page, a process stream, a diff, a help document, a search result are all buffer-substrate entities. Each carries additional entity-family facts describing its specific nature.

Substrate membership is a property of the fact space, not of the entity's essential type. An entity may satisfy multiple substrates or none.

Rendering of a substrate-satisfying entity is always a client concern. Different clients may render the same entity at different fidelities without affecting its substrate membership.

### 9.1 Projections

When a substrate operation targets an entity whose primary representation is not textual, the operation may act against a **projection** — a derived textual view of the entity.

Projections carry **structured annotations as first-class content**: regions, action targets, semantic markers travel alongside the rendered text, not on a separate out-of-band stream. This is Emacs text-properties promoted to a cross-service protocol.

Operations on projections are **projection-local by default**. Operations that would mutate the source representation must be explicitly routed to the source authority, which may accept or reject them. There are no implicit write-through projections.

Update granularity for non-quiescent sources (a live DOM under JS, a streaming log) is resolved at the protocol level: default is structured change events plus periodic fact snapshots, with per-service alternatives declared in lifecycle metadata. See protocols §3.2.
