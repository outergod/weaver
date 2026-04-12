# Architecture

## Core

Responsibilities:
- buffer management
- rendering
- input handling
- key dispatch
- workspace state
- event bus
- fact store (local view)
- UI projection

---

## Service Mesh

Services:
- run as separate processes
- communicate via message bus
- declare capabilities

Examples:
- git service
- LSP service
- search/index service
- project service

---

## Message Bus

Carries:
- events
- fact updates
- requests/responses

Supports:
- async communication
- streaming
- cancellation

---

## Fact Store

Core maintains a local, queryable view of:
- entities
- facts
- derived facts

---

## Failure Model

- service failure does not crash core
- degraded capabilities are visible
- retries and restarts supported

