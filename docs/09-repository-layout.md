# Repository layout

This repository is a monorepo. It separates concerns by project/workspace, not by repository.

- `docs/` — architecture, protocol, workflow, and open-question documents
- `core/` — Weaver core runtime and foundational services
- `buffers/` — `weaver-buffers` service: per-file content-backed publisher of `buffer/*` facts over the bus (slice 003)
- `git-watcher/` — `weaver-git-watcher` service: per-repository observer of `repo/*` facts (slice 002)
- `tui/` — minimal TUI (terminal UI) for testing
- `ui/` — user interface(s)
- `protocol/` — shared protocol/schema definitions
- `services/` — remote daemons and host-side services
- `tools/` — developer tooling and scripts

