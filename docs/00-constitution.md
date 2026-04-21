# Weaver Constitution (v0.2)

Weaver is a programmable, event-driven coordination substrate for symbolic work. It provides a shared semantic environment in which entities, facts, behaviors, services, and interfaces interact. Editor-like surfaces are one projection of this system, not its defining abstraction.

Weaver draws inspiration from systems like GNU Emacs, but generalizes the model beyond text editing into a multi-actor, distributed runtime.

This document defines the non-negotiable principles of the system.

## 1. No Monolithic Runtime

Weaver does not rely on a single embedded runtime as the universal medium of extension.

- Capabilities are distributed across services.
- Failures are isolated.
- The core remains responsive under partial failure.

The system must not require all extensibility to exist inside one privileged language image or process.

No single process, service, or actor may become a privileged, opaque locus of system behavior. The prohibition is not only architectural — it is a failure-domain guarantee.

---

## 2. Everything Is Introspectable

At all times, the user must be able to answer:

- What is happening?
- Why is it happening?
- What can I do here?
- Which service, behavior, or fact made this possible?

All meaningful system elements must expose:

- metadata
- provenance
- relationships
- applicability
- traceability (including causal context)

Opacity is a defect.

---

## 3. Entities Are Untyped

Entities are opaque, addressable references only.

Entities do not carry intrinsic type, class, or method identity.

Interpretation arises from:

- facts
- tags
- derived views
- behavior preconditions

No behavior may depend on privileged object essence.

---

## 4. Facts Over Objects

The system is modeled in terms of:

- entities
- facts about entities
- events
- behaviors
- authorities

There are no authoritative objects with owned methods.

Capabilities are not properties of objects. They emerge from current context.

---

## 5. Behavior Over Invocation

Direct function invocation is not the primary model of composition.

Instead:

- events occur
- facts hold
- behaviors react
- actions become applicable in context

Composition is reactive, contextual, and explainable.

---

## 6. Distribution Is First-Class

Weaver is inherently distributed.

- Services run independently.
- Communication is explicit.
- Knowledge may be partial.
- Latency is real.
- Failure is expected.

The system must remain usable and explainable under these conditions.

Distribution must not be hidden behind misleading assumptions of local immediacy.

---

## 7. Context Defines Capability

Available actions are not fixed by nominal type.

They are derived from:

- current facts
- current events
- current workspace context
- available services
- applicable behaviors

The primary interaction question is:

> What can I do here now?

---

## 8. Workspaces Are Lenses, Not Containers

Workspaces shape visibility, focus, and interpretation.

They do not own or imprison entities.

- entities remain globally addressable
- buffers may be compared across workspaces
- projects may be referenced across workspace boundaries

A workspace is a contextual lens, not an isolated box.

---

## 9. UI Is Not Authoritative

Weaver does not have a canonical UI.

User interfaces are independent clients that:

- subscribe to facts and events
- derive local views
- render state
- invoke actions

The core does not own rendering.

---

## 10. UI as Materialized View

A UI is an eventually consistent materialized view over system state.

- It may cache and derive local projections
- It may present state differently from other clients
- It may compute additional presentation-specific structures

UI state must not masquerade as shared semantic truth.

---

## 11. Shared State vs Local View

Weaver distinguishes between:

- shared semantic state (facts, events, relations)
- client-local view state (layout, visualization, transient projections)

Only shared semantic state participates in system-wide behavior.

Client-local view state remains outside the core model.

Shared semantic state may be partial and temporarily divergent across services, but must be reconcilable. Conflicts between authoritative contributions must be made explicit rather than silently resolved. Divergence without a path to reconciliation is a defect.

---

## 12. Composition Is First-Class

Users must be able to:

- define behaviors
- compose capabilities
- inspect compositions
- debug compositions
- understand why compositions fired or did not fire

Weaver must preserve user agency over system behavior.

---

## 13. Live Reflection

Users must be able to modify running behavior and observe the effect without restarting the system.

This applies to:

- behavior definitions
- composed actions
- applicability rules
- user-authored facts and fact families

Redefinition preserves session context. State authored by services and the core survives edits to composition.

The reflective loop is the runtime validation of §2 — a system that is introspectable but not live-modifiable is only half-introspectable.

---

## 14. Two Lanes of Extension

Extension flows through two lanes:

- **Governed citizenship.** Services declare fact families, claim authority, publish under versioned schemas. Governed facts are load-bearing for the shared semantic world.
- **User-scratch.** Composed behaviors observe, react, invoke actions, and may declare user-scratch fact families. User-scratch assertions carry non-authoritative provenance, cannot shadow governed facts, and are scoped by default.

A promotion path connects the lanes. Scratch that proves its value may be refactored into a governed service.

Both lanes are first-class. The ungoverned lane preserves user agency and velocity; the governed lane preserves the integrity of the shared semantic world.

---

## 15. Provenance Is Mandatory

Every fact, event, and derived action must be attributable.

The system must preserve:

- source
- authority
- causal chain
- freshness
- derivation

The user must be able to inspect why a fact exists and why an action is available.

---

## 16. Explainability Over Cleverness

Convenience, automation, and reactivity are valuable only if they remain inspectable and understandable.

When there is tension between:

- opaque convenience
- explainable behavior

Weaver prefers explainable behavior.

The system must remain legible to the user.

---

## 17. Multi-Actor Coherence

Weaver participates with heterogeneous actors — users, services, embedded behaviors, language hosts, and external agents. These are not disjoint categories, and actor identity is orthogonal to the extension lanes defined in §14.

The system must:

- record the originating actor in provenance for every fact, event, and action
- allow concurrent contributions from multiple actors without silent coalescence
- make conflicts and overlaps between contributions explicit and inspectable

Contributions from non-user actors MUST remain reversible and refusable by the user on inspection.

No actor may operate as an opaque authority. This closes the corresponding loophole in §1 at the actor level, just as §1 closes it at the process level.

The user is the final authority over shared semantic state. Non-user actors contribute under delegated powers that the user may constrain, revoke, or reverse.
