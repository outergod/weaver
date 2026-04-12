# Open Questions

This document tracks unresolved questions and productive tensions.

## 1. Composition Language

What should the first composition language be?

Possible directions include:

- small Scheme-like language
- constrained Lua-like language
- purpose-built declarative behavior language
- WASM-hosted user logic with a separate introspection layer

Key requirement: it must preserve inspectability.

---

## 2. Fact Semantics

How should facts be represented and indexed?

Questions include:

- flat triples versus richer records
- support for temporal versions
- support for confidence or freshness
- support for relation-valued facts

---

## 3. Tags Versus Predicates

How much should the system rely on explicit tags versus richer fact-pattern predicates?

Tags are convenient but may become ersatz nominal types if overused.

---

## 4. Derived Views

How should derived views be materialized?

Possibilities:

- recomputed on demand
- incrementally maintained
- cached in the core
- partially delegated to services

---

## 5. Event Loops and Stability

How should the system prevent accidental reactive loops?

Possible techniques include:

- causality tracking
- idempotence constraints
- loop guards
- transactional boundaries
- explicit one-shot versus persistent behaviors

---

## 6. Authority Boundaries

How strict should authority be?

Questions include:

- may multiple services publish competing facts in the same family
- how are conflicts represented
- what counts as speculative versus authoritative

---

## 7. Workspace Semantics

How should workspace facts influence applicability without becoming hidden containment?

This is central to preserving porous workspaces.

---

## 8. UI Intent Model

What shape should UI intents take so that services can influence interaction without owning rendering?

---

## 9. Trace Model

What is the minimal trace format that still makes the system understandable to users?

The trace system may become one of Weaver’s defining features.

---

## 10. Prototype Boundary

What is the smallest in-memory prototype that can honestly validate the ontology before introducing distribution?
