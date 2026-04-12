# System Model

Weaver models the world as entities, tags, facts, events, and behaviors.

## Entities

Opaque, stable references to things. 

Examples:
- buffer
- file
- project
- workspace
- symbol
- git repository

Entities have no intrinsic behavior.

---

## Facts

Facts describe entities.

Examples:
- (buffer e1)
- (file/path e1 "/foo/bar.rs")
- (language e1 :rust)
- (workspace/member e1 ws1)

Facts:
- are asserted or retracted
- may be authoritative or derived
- include provenance (who asserted them)

---

## Events

Events represent change or intent.

Examples:
- buffer/opened
- buffer/saved
- search/requested
- workspace/activated

Events:
- are transient
- may trigger behaviors

---

## Behaviors

Behaviors react to:
- events
- fact patterns

They may:
- emit events
- assert/retract facts
- request actions
- produce UI intents

---

## Authorities

Some services are authoritative for certain facts.

Examples:
- editor core → buffer state
- git service → repository state
- LSP → diagnostics

---

## Derived Facts

Facts may be:
- base (directly asserted)
- derived (computed)

Derived facts must:
- declare dependencies
- be recomputable

