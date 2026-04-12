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

## 7. Future Composition Language

The eventual composition language remains open.

However, it must satisfy these criteria:

- inspectable
- sandboxable
- event-aware
- fact-aware
- capable of expressing behavior preconditions and outputs
- able to define new user-visible actions
- integrated with the trace model

The system should not require users to give up legibility for expressive power.

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
