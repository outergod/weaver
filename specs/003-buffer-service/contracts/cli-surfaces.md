# CLI Surface Contracts — Slice 003

CLI flags and structured output shapes for the four binaries exercised by this slice: `weaver` (core), `weaver-tui`, `weaver-git-watcher` (slice 002, unchanged), and the new `weaver-buffers`. Per L2 P5 (amended) — CLI surfaces are **one-shot scripting** interfaces. Continuous machine consumers connect to the bus directly; see `contracts/bus-messages.md`.

## Binary: `weaver` (core) — delta from slice 002

### Unchanged

- Subcommands `status`, `inspect`, `run`, global flags (`--socket`, `--output`, etc.).
- `weaver --version` shape (human + JSON).
- Configuration schema + environment variables.
- Error rendering via `miette::Diagnostic`.
- Exit-code policy.

### Changed

**`weaver --version`** — JSON field `bus_protocol` advances `"0.2.0"` → `"0.3.0"`. Human output shows `bus protocol: v0.3.0`.

**`weaver status --output=json`** — fact entries now cover `buffer/*` families. `provenance.source` renders as `ActorIdentity::Service { service-id: "weaver-buffers", instance-id: ... }` for buffer facts (reusing the existing structured-identity rendering from slice 002 — no shape change).

Example status entry for a buffer-authored fact:

```json
{
  "fact": { "entity": 42, "attribute": "buffer/byte-size" },
  "value": { "type": "u64", "value": 12345 },
  "provenance": {
    "source": {
      "type": "service",
      "service-id": "weaver-buffers",
      "instance-id": "7b3c5a9e-1234-4abc-9def-111122223333"
    },
    "timestamp_ns": 205436000000,
    "causal_parent": 17
  }
}
```

**`weaver inspect <fact-key> --output=json`** — no shape change. Buffer-service-authored facts render with `asserting_service: "weaver-buffers"` and `asserting_instance: "<UUID>"` via the same additive shape slice 002 introduced:

```json
{
  "fact": { "entity": 42, "attribute": "buffer/dirty" },
  "source_event": 17,
  "asserting_service": "weaver-buffers",
  "asserting_instance": "7b3c5a9e-1234-4abc-9def-111122223333",
  "asserted_at_ns": 205436000000,
  "trace_sequence": 104
}
```

Human rendering:

```
fact:       42:buffer/dirty
source:     service weaver-buffers (instance 7b3c5a9e-1234-4abc-9def-111122223333)
event:      17
asserted:   2026-04-23 15:32:02.000 +0000
trace seq:  104
```

### REMOVED subcommands

**Breaking (MAJOR bump within `weaver` CLI surface):**

- `weaver simulate-edit <buffer-id>` — REMOVED. The `buffer/edited` event it emitted no longer exists in the protocol.
- `weaver simulate-clean <buffer-id>` — REMOVED. The `buffer/cleaned` event it emitted no longer exists.

Attempting to invoke either returns a clap parse error exit code (2) with message `"error: unrecognized subcommand '<simulate-edit|simulate-clean>'"`. Documented in `CHANGELOG.md` as a MAJOR CLI change.

**Migration note**: slice-003 e2e tests that previously exercised `simulate-edit`/`simulate-clean` are rewritten to drive the equivalent `buffer/open` + external-mutation paths through `weaver-buffers`. See `specs/003-buffer-service/quickstart.md` §Migration.

### Unchanged surfaces

`weaver run`, error rendering, configuration schema, `--socket` flag, `-o` / `--output` flags — all unchanged.

## Binary: `weaver-buffers` — NEW

### Invocation

```
weaver-buffers <PATH>... [OPTIONS]
```

### Positional arguments

| Argument | Description |
|---|---|
| `<PATH>...` | One or more paths to regular files. Each path is canonicalized at startup; relative paths resolved against `$PWD`. Duplicates (after canonicalization) are de-duplicated silently (FR-006a); a debug-level log records the dedup. Each unique canonical path becomes one buffer entity with authority claimed by this invocation. At least one path is required; zero-path invocation is a clap parse error. |

### Options

```
    --poll-interval=<duration>   Observation cadence. Default: 250ms.
                                 Accepts humantime-style values (e.g., 100ms, 1s, 2s500ms).
                                 0ms is rejected at parse time.
    --socket=<path>              Override the bus socket path.
                                 Default: $XDG_RUNTIME_DIR/weaver.sock
                                 Honors $WEAVER_SOCKET env var at parity with `weaver`.
    --output=<format>            human | json. Default: human.
                                 Applies to startup logs and the --version output.
-o  <format>                     Short alias for --output.
-v, --verbose                    Increase log verbosity. Repeatable (-v, -vv, -vvv).
    --version                    Print build provenance and exit.
-h, --help                       Print help and exit.
```

### Exit codes

| Code | Condition |
|---|---|
| 0 | Clean exit (SIGTERM / SIGINT received; all facts retracted; disconnect acknowledged) |
| 1 | Fatal startup error: any positional path is invalid (missing, unreadable, not a regular file, exceeds available memory) |
| 2 | Bus unavailable: core not reachable at the socket path, or handshake failed |
| 3 | Authority conflict: another buffer-service instance already claims one or more of the requested buffer entities |
| 10 | Internal error (unrecoverable; surfaces a `miette::Diagnostic`) |

*Asymmetry note (slice-002 F31 follow-up)*: server-sent `identity-drift` / `invalid-identity` errors currently exit code 10 (inherited from slice 002's reader-loop classification). A dedicated cross-service soundness slice will reclassify these as code 3 alongside the equivalent git-watcher fix.

### `weaver-buffers --version`

Same shape as `weaver --version` plus service-specific fields.

Human form:

```
weaver-buffers 0.1.0
  commit: a1b2c3d (dirty)
  built:  2026-04-23T15:05:00Z
  profile: debug
  bus protocol: v0.3.0
  service-id: weaver-buffers
```

JSON form:

```json
{
  "name": "weaver-buffers",
  "version": "0.1.0",
  "commit": "a1b2c3d",
  "dirty": true,
  "built_at": "2026-04-23T15:05:00Z",
  "profile": "debug",
  "bus_protocol": "0.3.0",
  "service_id": "weaver-buffers"
}
```

`service-id` is stable across invocations of a given binary; the per-invocation `instance-id` (UUID v4) is emitted in startup logs and embedded in every fact's provenance, not in `--version` output.

### Startup logs (human `--output=human`)

```
weaver-buffers 0.1.0 starting
  paths:
    /home/alex/code/weaver/core/src/lib.rs
    /home/alex/code/weaver/core/src/main.rs
  socket:   /run/user/1000/weaver.sock
  poll:     250ms
  instance: 7b3c5a9e-1234-4abc-9def-111122223333

[INFO] connected to core (bus protocol v0.3.0)
[INFO] published initial state:
         /home/alex/code/weaver/core/src/lib.rs  [18342 bytes]  clean
         /home/alex/code/weaver/core/src/main.rs [ 2156 bytes]  clean
         watcher/status = ready
```

### Startup logs (`--output=json`)

Each log line is a single JSON object on stderr. Structured via `tracing`'s JSON layer.

```json
{"timestamp":"2026-04-23T15:05:01.123Z","level":"INFO","target":"weaver_buffers::startup",
 "fields":{"paths":["/home/alex/code/weaver/core/src/lib.rs","/home/alex/code/weaver/core/src/main.rs"],
           "socket":"/run/user/1000/weaver.sock",
           "poll_interval":"250ms",
           "instance":"7b3c5a9e-1234-4abc-9def-111122223333"}}
```

### Error rendering

`miette::Diagnostic` per L2 P6. Structured errors name the fact-space condition.

**Path-not-openable at startup (code 1)**:

```
Error: buffer not openable at /home/alex/no-such-file: no such file or directory

  help: no fact (entity:<derived>, attribute:buffer/path) can be asserted.
        Point weaver-buffers at a regular file whose content you want to observe.

  code: WEAVER-BUF-001
```

JSON form (with `--output=json`):

```json
{
  "error": {
    "category": "buffer-not-openable",
    "code": "WEAVER-BUF-001",
    "message": "no such file or directory",
    "context": "/home/alex/no-such-file",
    "fact_key": null
  }
}
```

**Path is a directory (code 1)**:

```
Error: buffer not openable at /home/alex/code: path is a directory, not a regular file

  help: weaver-buffers only opens regular files in slice 003.
        Directory-level observation belongs to a future slice.

  code: WEAVER-BUF-002
```

**File too large / out of memory (code 1)**:

```
Error: buffer not openable at /home/alex/huge.log: file size exceeds available memory

  help: slice 003 reads each buffer's content fully into memory at open time.
        Streaming-open is not yet supported.

  code: WEAVER-BUF-003
```

**Authority conflict (code 3)**:

```
Error: buffer/* fact family for /home/alex/code/weaver/core/src/lib.rs
       is already claimed by weaver-buffers instance 2e1a4f8b-... (started 0:04:12 ago).

  help: only one weaver-buffers instance may own a given buffer entity at a time.
        Stop the other instance, or open a different file.

  code: WEAVER-BUF-004
```

## Binary: `weaver-tui` — delta from slice 002

### Unchanged

- Subcommands, keybindings, `--socket`, `--no-color`, `--version` JSON shape (except `bus_protocol` value), configuration schema, environment variables.

### Changed

**`weaver-tui --version`** — JSON field `bus_protocol` advances `"0.2.0"` → `"0.3.0"`.

**Render region extended.** The TUI gains a **Buffers** section beneath the existing **Repositories** section. The Buffers section lists each buffer entity the TUI currently knows about:

```
┌─ Weaver TUI ────────────────────────────────────────────────────────────────┐
│ Connection: ready (bus v0.3.0, core 0.3.0+a1b2c3d)                         │
│                                                                             │
│ Buffers:                                                                   │
│   /home/alex/code/weaver/core/src/lib.rs   [18342 bytes]  clean            │
│     by service weaver-buffers (inst 7b3c5a9e), event 17, 0.042s ago        │
│   /home/alex/code/weaver/core/src/main.rs  [ 2156 bytes]  dirty            │
│     by service weaver-buffers (inst 7b3c5a9e), event 23, 0.003s ago        │
│                                                                             │
│ Repositories:                                                              │
│   /home/alex/code/weaver  [on master] clean                                │
│     head: 4f7a8e3b...                                                      │
│     by service git-watcher (inst 2e1a4f8b), event 117, 0.310s ago          │
│                                                                             │
│ Commands: [i]nspect  [r]econnect  [q]uit                                   │
└─────────────────────────────────────────────────────────────────────────────┘
```

Display rules for the Buffers section:

- One row per buffer entity currently asserted (`buffer/path` and the other three bootstrap facts all present).
- `<path>` is rendered from the `buffer/path` fact's value, truncated with ellipsis on the left if it exceeds available width.
- `[<n> bytes]` is rendered from the `buffer/byte-size` fact's value (u64 decimal, right-aligned to a reasonable column width).
- Dirty badge: `dirty` or `clean` from the `buffer/dirty` fact's value. Replaced by `[observability lost]` when `buffer/observable=false`; dirty state is NOT shown in that case (it would be stale).
- When the TUI loses the core subscription, all buffer rows gain a `[stale]` badge per slice 002 convention; re-populated on reconnect.
- When no buffer service is attached or no buffers are open, the Buffers section shows `(none)`.
- Row ordering: deterministic by `(entity, attribute)` — same ordering rule as the Repositories section, so `[i]nspect` targets the visually-first displayed fact deterministically.
- Authoring-actor line: `by service weaver-buffers (inst <short-uuid>), event <id>, <t>s ago` — identical shape to the Repositories section's git-watcher line.

### In-TUI commands — unchanged

No new commands this slice. `[i]nspect` works on any currently-displayed fact including `buffer/*` facts.

### Commands NOT added (deferred)

- `[o]pen <path>` / file picker — slice 004 (requires editing keystrokes or at minimum a TUI-side `BufferOpen` event emission, which is out of scope).
- `[w]rite` / save — slice 004 (requires mutation).
- Buffer focus-cycling between open buffers — slice 005 (requires a focus-intent event; see FR-011a's forward-looking note).

### Unavailable-core rendering

Unchanged from slice 002 — the TUI shows `UNAVAILABLE` for the core with `[r]econnect` / `[q]uit` options. Buffer facts shown in the Buffers section are marked `[stale]` when the TUI loses its core subscription, and re-populated on reconnect.

## Binary: `weaver-git-watcher` — unchanged from slice 002

No CLI changes. `weaver-git-watcher --version` JSON field `bus_protocol` advances `"0.2.0"` → `"0.3.0"` (the only user-visible change).

## Configuration schema

Unchanged from slices 001/002. `weaver-buffers` respects the same `$XDG_CONFIG_HOME/weaver/config.toml` and the same `WEAVER_SOCKET` / `RUST_LOG` environment variables.

No new configuration keys are required this slice. The `--poll-interval` CLI flag is the single service-specific tuneable and is not persisted to config (operators invoke the service per-use, not as a long-lived configured daemon).

## Versioning policy

### `weaver` CLI + output

- **PATCH**: wording, log formatting, error-message text.
- **MINOR**: new subcommands, new flags, new JSON fields (additive only).
- **MAJOR**: removed subcommands, renamed subcommands, changed exit-code semantics. **This slice triggers MAJOR** (`simulate-edit` / `simulate-clean` removal). `CHANGELOG.md` entry required.

### `weaver-tui` output

- **PATCH**: rendering tweaks, color choices, text wrapping.
- **MINOR**: new rendered sections (e.g., the Buffers section added this slice), new keybindings (none this slice).
- **MAJOR**: keybinding removal, layout restructuring that breaks screen-reader / scraping compatibility (no explicit commitment yet).

### `weaver-buffers` CLI

Initial release at 0.1.0. Subsequent changes follow the same PATCH/MINOR/MAJOR scheme as `weaver`.

### `weaver-git-watcher` CLI

Unchanged at 0.1.0 (slice 002). Only the `bus_protocol` field value changes; the CLI shape is invariant.

`CHANGELOG.md` records every change per L2 P8, organized by surface.

## References

- `specs/003-buffer-service/spec.md` — user stories, FRs, SCs, Clarifications 2026-04-23.
- `specs/003-buffer-service/data-model.md` — `BufferState`, entity-id derivation, lifecycle, validation rules.
- `specs/003-buffer-service/contracts/bus-messages.md` — wire contract for the shared protocol bump.
- `specs/003-buffer-service/research.md` — library and strategy decisions.
- `docs/02-architecture.md §7.1` — latency classes referenced by `--poll-interval` defaults.
- `docs/05-protocols.md §5` — lifecycle vocabulary used by `watcher/status`.
