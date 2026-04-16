# Weaver

Weaver is a programmable, event-driven editing system. Weaver is conceived as a spiritual evolution of GNU Emacs.

It replaces monolithic extensibility with a distributed, introspectable model built from:

- entities
- facts
- events
- behaviors
- independent services

In Weaver, workflows do not arise from objects with methods or a single embedded runtime. They emerge from behaviors reacting to contextual fact patterns across a system of cooperating services.

## Repository layout

This repository is a monorepo. It separates concerns by project/workspace, not by repository.

- `docs/` — architecture, protocol, workflow, and open-question documents
- `core/` — Weaver core runtime and foundational services
- `ui/` — user interface(s)
- `protocol/` — shared protocol/schema definitions
- `services/` — remote daemons and host-side services
- `tools/` — developer tooling and scripts
