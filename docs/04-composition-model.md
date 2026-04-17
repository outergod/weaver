# Composition Model

Weaver treats composition as a first-class, user-facing capability.

## 1. What Composition Means

Composition in Weaver means defining new behavior from existing elements such as:

- events
- fact predicates
- action proposals
- service requests
- derived facts
- UI intents

Composition is not primarily method chaining or object extension.

It is the controlled creation of new reactive behavior.

---

## 2. Behavior Shape

A behavior has, at minimum:

- a name
- triggering event conditions
- contextual fact predicates
- optional service-availability predicates
- explicit outputs

Outputs may include:

- asserted facts
- retracted facts
- emitted events
- action proposals
- structured UI intents

Example:

```lisp
(behavior format-on-save
  (on buffer/saved
    (when (language $buffer :rust)
      (emit formatter/request))))
```

---

## 3. Applicability Over Ownership

A composed behavior becomes relevant when its conditions match current context.

It does not attach itself to an entity as a method.

This preserves additive extensibility and avoids invasive mutation of central ontology.

---

## 4. Derived Actions

A user-visible action may itself be a derived result of one or more behaviors.

For example:

- a compare action may become available when two relevant entities are marked and comparable
- a git action may become available when an entity participates in a project associated with a repository and the git service is available

Actions are therefore contextual projections, not object members.

---

## 5. Introspection of Composition

Users must be able to inspect:

- behavior definitions
- triggering conditions
- matched facts
- emitted outputs
- causal traces
- current enablement or disablement state

A composition that cannot be understood is incomplete.

---

## 6. Debugging Requirements

The system must make it possible to answer:

- why did this behavior fire
- why did it not fire
- which facts matched
- which service supplied those facts
- what outputs were produced
- what downstream effects followed

Debugging is part of composition design.

---

## 7. Composition Language: Steel

Weaver's composition language is **Steel** — a Scheme implementation in Rust, embedded in the core process.

Rationale:

- homoiconic, live-evaluable, macro-capable — preserves the reflective loop (constitution §13)
- structural support for behaviors, fact patterns, event dispatch
- ergonomic Rust interop for host-exposed primitives
- sandboxable via host-controlled capability exposure
- available today; existing proof-of-concept demonstrates the Rust-primitive / Scheme-composition split

Commitments that follow from choosing Steel:

- **Performance-critical primitives live in Rust services, not in Steel.** Steel is composition glue; hot paths are service work.
- **Steel is version-pinned.** Weaver tracks a specific Steel release; upstream changes are adopted deliberately.
- **Service scaffolding is a project responsibility.** Writing a minimum-viable Rust service must approach `defun`-level ceremony — templates, trivial registration, hot reload. Without this, the cost of promoting scratch to service erodes extension velocity.

Steel is the only composition language. Any language-based extensibility for specific fact families (embedded DSLs, domain languages) is built as a service that accepts its own sources via the bus.

---

## 8. Composition and UI

Composition defines behavior over semantic state, not rendering.

Composed behaviors may:

- produce facts
- emit events
- propose actions
- produce UI intents (optional, structured)

UI intents must remain declarative and inspectable.

Rendering decisions remain the responsibility of clients.

---

## 9. UI Intents

Behaviors may emit UI intents such as:

- "highlight this entity"
- "focus this entity"
- "present these results"

These are suggestions, not commands.

Each client may interpret or ignore them.

---

## 10. Extension Lanes

Extension flows through two lanes (constitution §14).

### 10.1 Governed

Services are the governed lane. They declare fact families, claim authority, publish under namespaced and versioned schemas, and carry authoritative provenance. Failures degrade explicitly.

### 10.2 User-Scratch

Composed behaviors in Steel are the user-scratch lane. They:

- react to events and predicate on facts
- invoke actions and emit UI intents
- may declare user-scratch fact families and assert into them

User-scratch assertions are subject to four governance rules:

1. **Mandatory provenance tagging.** Every user-scratch assertion carries `source: user-scratch:<file>:<location>` (or equivalent) in its provenance metadata — no exceptions.
2. **Never authoritative.** User-scratch facts cannot claim authority over a fact family and cannot shadow governed authorities' claims.
3. **Scoped lifetime by default.** User-scratch facts are session- or behavior-scoped unless the author explicitly declares them persistent.
4. **Explicit trace surfacing.** When a behavior predicates on a user-scratch fact, the `why?` channel surfaces the non-authoritative provenance; traces make governed vs. user-scratch contributions legible.

### 10.3 Promotion Path

A user-scratch behavior that earns adoption may be promoted:

1. **Scratch** — Steel-authored behavior, user-scratch facts, local to the author.
2. **Package** — schema drafted, intended authority declared but still marked speculative, shareable.
3. **Governed** — formal authority claimed, namespace and version registered, compatibility burden accepted.

Promotion is deliberate, not automatic. Tooling must inspect a user-scratch family and emit a schema draft plus service stub — without this, promotion does not happen and scratch calcifies.

---

## 11. Reflective Loop

Behaviors and user-scratch fact families may be redefined in a running session.

Redefinition preserves:

- authoritative fact state
- active subscriptions where schema-compatible
- action-entity identity where `(action-type, target)` is unchanged

Redefinition invalidates:

- cached derived facts that depended on the prior behavior
- applicability computations pending at the moment of redefinition

### 11.1 Behavior-Authored State on Reload

On reload, state authored by the prior version of a behavior is **strictly re-derived by default**:

- user-scratch facts asserted by the prior version are retracted
- user-scratch fact families declared by the prior version are dropped unless the new version re-declares them
- action entities whose applicability derivations came from the prior version re-evaluate against the new version on the next relevant event

Authors who need state to outlive the behavior's current source version mark the relevant fact family as `:reload-preserve` explicitly. This opt-in turns strict re-derivation into per-family author-declared preservation.

This rule keeps the reflective loop legible: after reload, the fact space's author-sourced content matches the current behavior source, with no ghosts from earlier versions.

The reflective loop is the runtime validation of the composition model. If live redefinition breaks, composition is not truly first-class.

---

## 12. Event-Loop Stability

Behaviors firing behaviors risks accidental reactive loops. The system uses two committed mechanisms to contain them.

### 12.1 Causality Tracking and Loop-Depth Guard

Every event carries causal-parent provenance (protocols §2). A loop-depth guard terminates a behavior cascade that exceeds a configured depth; the terminated cascade is recorded in the trace with its full causal chain for inspection.

### 12.2 One-Shot vs Persistent Behaviors

Behaviors declare their applicability lifetime:

- **persistent** — the default; the behavior remains applicable as long as its preconditions hold and its source is registered
- **one-shot** — the behavior fires once per matching event and is not re-evaluated until explicitly re-armed

One-shot annotations prevent reactive self-triggering where a behavior's outputs satisfy its own preconditions.

### 12.3 Deferred Mechanisms

Two further stability mechanisms remain available but are not committed for the MVP:

- **idempotence contracts** — behaviors declaring themselves safe to re-fire with the same inputs
- **transactional boundaries** — behavior cascades grouped into atomic units that commit or roll back

They remain open (see 07-open-questions §5). They become relevant if causality tracking and one-shot annotations prove insufficient in practice.
