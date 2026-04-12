# Weaver Constitution (v0.1)

Weaver is a programmable, event-driven editing system in which workflows emerge from interactions between independent services. Weaver is conceived as a spiritual evolution of GNU Emacs.

This document defines the non-negotiable principles of the system.

## 1. No Monolithic Runtime

Weaver does not rely on a single embedded runtime (e.g. a Lisp machine).

- Capabilities are distributed across services.
- Failures are isolated.
- The system remains responsive under partial failure.

---

## 2. Everything Is Introspectable

At all times, the user must be able to answer:

- What is happening?
- Why is it happening?
- What can I do here?

All system elements expose:
- metadata
- provenance
- capabilities
- relationships

---

## 3. Facts Over Objects

The system is modeled as:

- entities
- tags
- facts about entities
- events
- behaviors

There are no authoritative “objects with methods”.

Capabilities emerge from:
- facts
- behaviors reacting to those facts

---

## 4. Behavior Over Invocation

Function calls are not the primary abstraction.

Instead:
- behaviors react to events and fact patterns
- actions emerge from context
- composition is declarative and reactive

---

## 5. Distribution Is First-Class

The system is inherently distributed:

- services run independently
- communication is explicit (message bus)
- latency and partial knowledge are acknowledged realities

The system must:
- degrade gracefully
- remain explainable under distribution

---

## 6. Context Defines Capability

Available actions are not static.

They are derived from:
- current facts
- active entities
- available services

The system answers:
> “What can I do here?” dynamically.

---

## 7. Workspaces Are Lenses, Not Containers

Workspaces:
- shape visibility and context
- do not isolate entities

All entities remain globally addressable.

---

## 8. UI Is a Projection, Not an Authority

No service owns UI.

- Services provide data and intent
- Core renders UI consistently

---

## 9. Composition Is a First-Class Capability

Users must be able to:

- define new behaviors
- compose existing capabilities
- introspect and debug compositions

---

## 10. The System Must Remain Explainable

Every action must be traceable:

- which facts triggered it
- which behavior executed
- which service participated

Opacity is a bug.

