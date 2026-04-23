# weaver-buffers

Weaver's first **content-backed service** on the bus. Opens one or more files named on the CLI, holds each file's byte content in memory (conceptually a `:content` component per `docs/01-system-model.md §2.4`), and publishes a small set of derived facts over the bus under an `ActorIdentity::Service`.

**Published fact families** (per opened buffer entity, `buffer/*` authority held exclusively by this service):

- `buffer/path` — canonical absolute path (`FactValue::String`).
- `buffer/byte-size` — in-memory content length (`FactValue::U64`).
- `buffer/dirty` — `true` iff in-memory content differs from on-disk content at the last observation (`FactValue::Bool`).
- `buffer/observable` — `false` during transient read failure for that buffer; edge-triggered (`FactValue::Bool`).

**Lifecycle**: publishes `watcher/status` on its own instance entity (`started` → `ready` → [↔ `degraded`] → `unavailable` → `stopped`). Service-level and **orthogonal** to per-buffer `buffer/observable` (Clarification 2026-04-23).

**Buffer content NEVER appears as a fact value**, directly or indirectly. No preview, no digest, no range fetch on the wire. Content stays in memory + on disk; only derivations cross the bus. This is the slice's defining invariant per constitution §2.4.

## Usage

```bash
weaver-buffers <PATH>... [--socket=<path>] [--poll-interval=<duration>] [--output=human|json] [-v|-vv|-vvv]
```

Examples:

```bash
# open one file against a running core
weaver-buffers ./notes.md

# open several files; duplicates de-duplicate at parse time after canonicalization
weaver-buffers ./a.txt ./b.txt ./c.txt

# custom poll cadence (default 250ms)
weaver-buffers ./file --poll-interval=500ms
```

**Exit codes**:

- `0` — clean exit (SIGTERM / SIGINT; all facts retracted).
- `1` — startup failure (path invalid, unreadable, directory, exceeds memory).
- `2` — bus unavailable (core not reachable; handshake failed; core EOF).
- `3` — authority conflict (another instance already claims one of the buffers).
- `10` — internal error (`miette::Diagnostic`).

## Spec

See `specs/003-buffer-service/` for the full feature specification, implementation plan, wire contracts, and data model.

## Status

**Slice 003 Phase 1 scaffold** — binary prints a placeholder string and exits. Real behavior lands across Phases 2–6 per `specs/003-buffer-service/tasks.md`.
