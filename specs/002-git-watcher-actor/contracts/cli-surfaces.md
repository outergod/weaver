# CLI Surface Contracts — Slice 002

CLI flags and structured output shapes for the three binaries exercised by this slice: `weaver` (core), `weaver-tui`, and the new `weaver-git-watcher`. Per L2 P5 (amended) — CLI surfaces are **one-shot scripting** interfaces. Continuous machine consumers connect to the bus directly; see `contracts/bus-messages.md`.

## Binary: `weaver` (core) — delta from slice 001

### Unchanged

- Subcommands, exit codes, global flags.
- `weaver --version` human + JSON output shape.
- `weaver status`, `weaver simulate-edit`, `weaver simulate-clean` shapes.
- Configuration schema + environment variables.
- Error rendering via `miette::Diagnostic`.

### Changed

**`weaver --version`** — JSON adds `bus_protocol: "0.2.0"` (was `0.1.0` in slice 001). Human output shows `bus protocol: v0.2.0`.

**`weaver status --output=json`** — fact entries' `provenance.source` renders as a structured actor identity object, not a tagged string.

Before (slice 001):
```json
"provenance": {
  "source": { "type": "behavior", "id": "core/dirty-tracking" },
  "timestamp_ns": 12340000000,
  "causal_parent": 42
}
```

After (slice 002, this surface is already structured — same JSON shape, but `source.type` now accepts `"service"` for external actors):
```json
"provenance": {
  "source": {
    "type": "service",
    "service-id": "git-watcher",
    "instance-id": "2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90"
  },
  "timestamp_ns": 143522000000,
  "causal_parent": 117
}
```

**Note:** Slice 001 already used adjacent-tagged JSON for `SourceId`; the additive change this slice is that `"type": "service"` becomes a valid variant and the variant carries `service-id` + `instance-id` fields rather than a single `id`. Existing JSON consumers that only ever saw `"behavior"` / `"core"` / `"tui"` continue to deserialize those variants unchanged.

**`weaver inspect <fact-key> --output=json`** — the `asserting_behavior` field is augmented to handle non-behavior authors cleanly. When the asserting actor is an in-core behavior, the field remains (slice 001 back-compat). When the actor is a service, two new fields appear and `asserting_behavior` is omitted:

Behavior-asserted fact (slice 001 shape unchanged):
```json
{
  "fact": { "entity": 1, "attribute": "buffer/dirty" },
  "source_event": 42,
  "asserting_behavior": "core/dirty-tracking",
  "asserted_at_ns": 12340000000,
  "trace_sequence": 17
}
```

Service-asserted fact (new shape):
```json
{
  "fact": { "entity": 7, "attribute": "repo/dirty" },
  "source_event": 117,
  "asserting_service": "git-watcher",
  "asserting_instance": "2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90",
  "asserted_at_ns": 143522000000,
  "trace_sequence": 204
}
```

Human rendering mirrors the structure:
```
fact:       7:repo/dirty
source:     service git-watcher (instance 2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90)
event:      117
asserted:   2026-04-21 14:32:02.000 +0000
trace seq:  204
```

**Compatibility**: the JSON shape is additive — existing consumers reading `asserting_behavior` get `None` / absent for service-authored facts and must handle that. Documented in `CHANGELOG.md`.

**`weaver status --output=json`** also gains `facts[*].value.type = "string"` as a first-class value type if it wasn't previously surfaced — `repo/head-commit` is a `FactValue::String`, required for rendering (existing `FactValue` variants already include `String`, so no new enum variant).

### Unchanged surfaces

`weaver run`, error rendering, configuration schema, `--socket` flag, `-o` / `--output` flags — all unchanged.

## Binary: `weaver-git-watcher` — NEW

### Invocation

```
weaver-git-watcher <REPOSITORY-PATH> [OPTIONS]
```

### Positional arguments

| Argument | Description |
|---|---|
| `<REPOSITORY-PATH>` | Path to the git repository's working-tree root. Canonicalized at startup; relative paths resolved against `$PWD`. Must exist and contain a valid `.git` directory (or equivalent). |

### Options

```
    --poll-interval=<duration>   Observation cadence. Default: 250ms.
                                 Accepts humantime-style values (e.g., 100ms, 1s, 2s500ms).
    --socket=<path>              Override the bus socket path.
                                 Default: $XDG_RUNTIME_DIR/weaver.sock
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
| 1 | Fatal startup error: repository path invalid, not a git repository, permission denied |
| 2 | Bus unavailable: core not reachable at the socket path, or handshake failed |
| 3 | Authority conflict: another watcher already claims the repository's fact family |
| 10 | Internal error (unrecoverable; surfaces a `miette::Diagnostic`) |

### `weaver-git-watcher --version`

Same shape as `weaver --version` plus watcher-specific fields.

Human form:
```
weaver-git-watcher 0.1.0
  commit: a1b2c3d (dirty)
  built:  2026-04-21T14:05:00Z
  profile: debug
  bus protocol: v0.2.0
  service-id: git-watcher
```

JSON form:
```json
{
  "name": "weaver-git-watcher",
  "version": "0.1.0",
  "commit": "a1b2c3d",
  "dirty": true,
  "built_at": "2026-04-21T14:05:00Z",
  "profile": "debug",
  "bus_protocol": "0.2.0",
  "service_id": "git-watcher"
}
```

`service-id` is stable across invocations of a given binary; the per-invocation `instance-id` is emitted in startup logs (and embedded in every fact's provenance), not in `--version` output.

### Startup logs (human `--output=human`)

```
weaver-git-watcher 0.1.0 starting
  repository: /home/alex/code/github.com/example/project
  socket:     /run/user/1000/weaver.sock
  poll:       250ms
  instance:   2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90

[INFO] connected to core (bus protocol v0.2.0)
[INFO] published initial state:
         repo/path          = /home/alex/code/github.com/example/project
         repo/head-commit   = 4f7a8e3b...
         repo/state/on-branch = main
         repo/dirty         = false
         repo/observable    = true
         watcher/status     = ready
```

### Startup logs (`--output=json`)

Each log line is a single JSON object on stderr. Structured via `tracing`'s JSON layer.

```json
{"timestamp":"2026-04-21T14:05:01.123Z","level":"INFO","target":"weaver_git_watcher::startup",
 "fields":{"repository":"/home/alex/code/github.com/example/project",
           "socket":"/run/user/1000/weaver.sock",
           "poll_interval":"250ms",
           "instance":"2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90"}}
```

### Error rendering

`miette::Diagnostic` per L2 P6. Structured errors name the fact-space condition:

```
Error: repo not observable at /home/alex/no-such-dir: not a git repository

  help: no fact (entity:<watcher-instance>, attribute:repo/path) can be asserted.
        Point weaver-git-watcher at a directory whose git state you want to observe.

  code: WEAVER-GW-001
```

JSON form (with `--output=json`):
```json
{
  "error": {
    "category": "repo-not-observable",
    "code": "WEAVER-GW-001",
    "message": "not a git repository",
    "context": "/home/alex/no-such-dir",
    "fact_key": null
  }
}
```

### Authority-conflict error

```
Error: repo/* fact family for /home/alex/code/github.com/example/project
       is already claimed by watcher instance 2e1a4f8b-... (started 0:04:12 ago).

  help: only one weaver-git-watcher may observe a repository at a time.
        Stop the other instance, or watch a different repository.

  code: WEAVER-GW-003
```

Exit code 3.

## Binary: `weaver-tui` — delta from slice 001

### Unchanged

- Subcommands, keybindings, `--socket`, `--no-color`, `--version` shape, configuration schema, environment variables.

### Changed

**Render region extended.** The TUI gains a **repositories** section beneath the existing **facts** section. The repositories section lists each repository entity the TUI currently knows about, with its observed state:

```
┌─ Weaver TUI ────────────────────────────────────────────────────────┐
│ Connection: ready (bus v0.2.0, core 0.2.0+a1b2c3d)                  │
│                                                                     │
│ Buffers:                                                            │
│   buffer/dirty(EntityRef(1)) = true                                 │
│     by behavior core/dirty-tracking, event 42, 0.142s ago           │
│                                                                     │
│ Repositories:                                                       │
│   /home/alex/code/github.com/example/project  [on main] clean       │
│     head: 4f7a8e3b...                                               │
│     by service git-watcher (inst 2e1a4f8b), event 117, 0.310s ago   │
│                                                                     │
│ Commands: [e]dit  [c]lean  [i]nspect  [r]econnect  [q]uit           │
└─────────────────────────────────────────────────────────────────────┘
```

Display rules for the repositories section:

- The state badge reflects which `repo/state/*` variant is asserted: `[on <branch>]`, `[detached <short-sha>]`, `[unborn <intended>]`, or `[state unknown]` if none asserted (watcher not yet ready or degraded).
- Dirty status: `dirty` / `clean`. If `repo/observable = false`, the badge is `[observability lost]` and no dirty state is rendered.
- The `by` line renders the authoring actor's kind + service-id + a short instance suffix.

When the watcher is not attached, the Repositories section shows `(none)`. When the watcher is degraded, its most recent `repo/state/*` value is shown with a `[stale]` marker next to it.

### In-TUI commands — unchanged

No new commands in this slice. `[i]nspect` works on any currently-displayed fact including `repo/*` facts.

### Unavailable-core rendering

Unchanged from slice 001 — the TUI shows `UNAVAILABLE` for the core with `[r]econnect` / `[q]uit` options. Repository facts shown in the Repositories section are marked `[stale]` when the TUI loses the core's subscription stream, and re-populated on reconnect.

## Configuration schema

Unchanged from slice 001. `weaver-git-watcher` respects the same `$XDG_CONFIG_HOME/weaver/config.toml` and the same `WEAVER_SOCKET` / `RUST_LOG` environment variables.

No new configuration keys are required this slice. The `--poll-interval` CLI flag is the single watcher-specific tuneable and is not persisted to config (operators invoke the watcher per-use, not as a long-lived configured service).

## Versioning policy

### `weaver` CLI + output

- **PATCH**: wording, log formatting, error-message text.
- **MINOR**: new subcommands, new flags, new JSON fields (additive only). This slice's `asserting_service` / `asserting_instance` additions are MINOR.
- **MAJOR**: removed fields, renamed subcommands, changed exit-code semantics.

### `weaver-tui` output

- **PATCH**: rendering tweaks, color choices, text wrapping.
- **MINOR**: new rendered sections (e.g., the Repositories section added this slice), new keybindings (none this slice).
- **MAJOR**: keybinding removal, layout restructuring that breaks screen-reader / scraping compatibility (no explicit commitment yet; noted for future slices).

### `weaver-git-watcher` CLI

Initial release at 0.1.0. Subsequent changes follow the same PATCH/MINOR/MAJOR scheme as `weaver`.

`CHANGELOG.md` records every change per L2 P8.

## References

- `specs/002-git-watcher-actor/spec.md` — user stories, FRs, SC.
- `specs/002-git-watcher-actor/data-model.md` — `ActorIdentity`, `repo/*` families, lifecycle.
- `specs/002-git-watcher-actor/contracts/bus-messages.md` — wire contract for the shared provenance shape.
- `docs/02-architecture.md §7.1` — latency classes referenced by `--poll-interval` defaults.
- `docs/05-protocols.md §5` — lifecycle vocabulary used by `watcher/status`.