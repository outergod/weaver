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
- derivation information if applicable

Facts should be small, explicit, and composable.

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

---

## 8. UI Intents

UI intents are structured suggestions emitted by behaviors.

They:

- reference entities and facts
- describe desired presentation effects
- are not authoritative

Examples:

- highlight entity
- focus entity
- display result set
- reveal relation

Interpretation is left to clients.
