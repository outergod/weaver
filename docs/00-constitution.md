# Weaver Constitution (v0.1)

Weaver is a programmable, event-driven editing system in which workflows emerge from interactions between independent services. Weaver is conceived as a spiritual evolution of GNU Emacs.

This document defines the non-negotiable principles of the system.

## 1. No Monolithic Runtime

Weaver does not rely on a single embedded runtime as the universal medium of extension.

- Capabilities are distributed across services.
- Failures are isolated.
- The core remains responsive under partial failure.

The system must not require all extensibility to exist inside one privileged language image or process.

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
- traceability

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

## 13. Provenance Is Mandatory

Every fact, event, and derived action must be attributable.

The system must preserve:

- source
- authority
- causal chain
- freshness
- derivation

The user must be able to inspect why a fact exists and why an action is available.

---

## 14. Explainability Over Cleverness

Convenience, automation, and reactivity are valuable only if they remain inspectable and understandable.

When there is tension between:

- opaque convenience
- explainable behavior

Weaver prefers explainable behavior.

The system must remain legible to the user.
