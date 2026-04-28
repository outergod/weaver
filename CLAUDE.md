<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:

- `specs/005-buffer-save/plan.md` — Buffer Save slice (current)
- `specs/005-buffer-save/spec.md` — feature specification (with 2 clarifications, 2026-04-27)
- `specs/005-buffer-save/research.md` — Phase 0 decisions (BusMessage<E> generic shape, inode capture via MetadataExt, atomic_write_with_hooks for I/O fault injection, EventOutbound↔Event relationship, stamped-EventId counter on TraceStore, tempfile naming with UUID v4 suffix, parent-dir fsync, §28(a) atomic migration, BufferSaveOutcome taxonomy, in-process inspect-lookup reuse, §28 doc lockstep)
- `specs/005-buffer-save/data-model.md`, `specs/005-buffer-save/contracts/`, `specs/005-buffer-save/quickstart.md` — Phase 1 design

Prior slices (shipped):

- `specs/001-hello-fact/` — Hello, fact slice (merged)
- `specs/002-git-watcher-actor/` — Git-Watcher Actor slice (merged)
- `specs/003-buffer-service/` — Buffer Service slice (merged)
- `specs/004-buffer-edit/` — Buffer Edit slice (merged via PR #11, 2026-04-27)

Architecture and engineering constitutions:

- `docs/00-constitution.md` — L1 architectural constitution (v0.2; §17 Multi-Actor Coherence)
- `.specify/memory/constitution.md` — L2 engineering constitution (currently v0.7.0)
- `AGENTS.md` — orientation for contributors and agents
<!-- SPECKIT END -->
