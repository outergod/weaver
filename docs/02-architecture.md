# Architecture

Weaver is composed of a small local core and a set of cooperating services.

## 1. Core Responsibilities

The core is responsible for:

- entity and fact management
- event processing
- behavior execution
- action invocation
- service coordination
- authority enforcement
- trace and provenance tracking
- maintaining a local semantic model of the system

The core is not responsible for rendering UI.

---

## 2. Services

Capabilities beyond the editing core are provided by services.

Typical services may include:

- project service
- git service
- search/index service
- language service bridge
- task/test service
- file watcher service

Services:

- run independently
- communicate explicitly
- declare capabilities
- publish facts and events
- participate in a shared behavioral system

No service is entitled to become a replacement monolith.

---

## 3. Message Bus

The system communicates through an explicit bus or transport layer carrying:

- events
- fact assertions
- fact retractions
- requests
- responses
- streams
- health and lifecycle signals

The bus must support:

- asynchronous operation
- cancellation
- structured errors
- streaming where appropriate
- provenance preservation

---

## 4. Fact Space

The core maintains a local, queryable view of shared semantic state.

This includes:

- authoritative facts
- derived semantic relations
- service-provided state
- interaction-relevant projections

This model represents meaning, not presentation.

---

## 5. Authority and Ownership

Each authoritative fact family must have a declared owner.

Examples:

- buffer-open state → core
- project root mapping → project service
- repository branch and hunk state → git service
- diagnostics → language service bridge

This avoids contradictory canonical claims.

Derived or speculative facts may still exist, but must be marked accordingly.

---

## 6. Failure Model

Weaver must degrade gracefully.

If a service fails:

- the core remains responsive
- dependent capabilities become unavailable or stale in an explicit way
- traces preserve what happened
- restart or reattachment is possible

Failure must not become silent semantic corruption.

---

## 7. Latency Model

The system must acknowledge that not all actions are local or immediate.

Therefore:

- editing actions may be local and immediate
- service-dependent actions may be asynchronous
- partial results may appear before complete results
- pending and stale states must be representable

Latency is part of the architecture, not an implementation embarrassment.

---

## 8. UI Boundary

User interfaces are independent clients of the system.

They:

- subscribe to facts and events
- query semantic state
- invoke actions
- derive local views
- render presentation

The core may expose semantic projections (e.g. relations, applicability), but:

- it does not define layout
- it does not define rendering
- it does not enforce a canonical visual structure
