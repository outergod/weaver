# CLI Surface Contracts

CLI flags and structured output shapes for `weaver` (core binary) and `weaver-tui` (TUI binary). Per L2 P5 (amended) — these are **one-shot scripting** surfaces. Continuous machine consumers connect to the bus directly; see `bus-messages.md`.

## Binary: `weaver` (core)

### Subcommands

| Subcommand | Purpose | Exit code semantics |
|---|---|---|
| `weaver run` | Start the core process; block until SIGINT/SIGTERM | 0 on clean exit; non-zero on startup failure |
| `weaver --version` (or `weaver -V`) | Print build provenance | 0 always |
| `weaver status` | One-shot snapshot of core lifecycle + currently-asserted facts | 0 if core reachable; 2 if core unavailable |
| `weaver inspect <fact-key>` | One-shot inspection of a fact's provenance | 0 on found; 2 on not-found; 3 on core unavailable |
| `weaver simulate-edit <buffer-id>` | Publish a `buffer/edited` event for a synthetic buffer | 0 on accepted; 2 on core unavailable |
| `weaver simulate-clean <buffer-id>` | Publish a `buffer/cleaned` event for a synthetic buffer | 0 on accepted; 2 on core unavailable |

### Global flags

```
-v, --verbose            increase log verbosity (repeatable: -v, -vv, -vvv)
    --output=<format>    json | human (default: human); applies to commands that produce structured output
    --socket=<path>      override the default bus socket path (default: $XDG_RUNTIME_DIR/weaver.sock)
```

### Output shapes (`--output=json`)

#### `weaver --version`

Human form:
```
weaver 0.1.0
  commit: a1b2c3d (dirty)
  built:  2026-04-19T15:30:00Z
  profile: debug
  bus protocol: v0.1.0
```

JSON form:
```json
{
  "name": "weaver",
  "version": "0.1.0",
  "commit": "a1b2c3d",
  "dirty": true,
  "built_at": "2026-04-19T15:30:00Z",
  "profile": "debug",
  "bus_protocol": "0.1.0"
}
```

All fields per L2 P11.

#### `weaver status --output=json`

```json
{
  "lifecycle": "ready",
  "uptime_ns": 12345678900,
  "facts": [
    {
      "key": { "entity": 1, "attribute": "buffer/dirty" },
      "value": { "type": "bool", "value": true },
      "provenance": {
        "source": { "type": "behavior", "id": "core::dirty_tracking" },
        "timestamp_ns": 12340000000,
        "causal_parent": 42
      }
    }
  ]
}
```

The shape **mirrors the bus vocabulary** per L2 P5 — a future agent service that subscribes to `FactAssert` over the bus sees the same field names, just CBOR-encoded.

If the core is unavailable:
```json
{
  "lifecycle": "unavailable",
  "error": "core not reachable at /run/user/1000/weaver.sock"
}
```

Exit code 2.

#### `weaver inspect <fact-key> --output=json`

Fact key syntax: `<entity-id>:<attribute>` (e.g., `1:buffer/dirty`).

Found:
```json
{
  "fact": { "entity": 1, "attribute": "buffer/dirty" },
  "source_event": 42,
  "asserting_behavior": "core::dirty_tracking",
  "asserted_at_ns": 12340000000,
  "trace_sequence": 17
}
```

Not found:
```json
{
  "fact": { "entity": 1, "attribute": "buffer/dirty" },
  "error": "FactNotFound"
}
```

Exit code 2.

#### `weaver simulate-edit <buffer-id>` and `weaver simulate-clean <buffer-id>`

Output (success):
```json
{
  "event_id": 43,
  "name": "buffer/edited",
  "target": 1,
  "submitted_at_ns": 12345000000
}
```

Note: success means *event submitted to the bus*; the `dirty` fact arrival is observable separately via subscription (TUI) or `weaver status`.

### Error rendering (L2 P6)

All errors are `miette::Diagnostic` types with `--output=human` rendering by default and JSON rendering under `--output=json`. JSON error shape:

```json
{
  "error": {
    "category": "core_unavailable",
    "code": "WEAVER-002",
    "message": "Core not reachable at /run/user/1000/weaver.sock",
    "context": "tried to connect for `weaver status`",
    "fact_key": null
  }
}
```

`fact_key` is populated for errors that reference fact-space state (per L2 P6 example):
```json
{
  "error": {
    "category": "fact_not_found",
    "code": "WEAVER-201",
    "message": "no fact (entity:1, attribute:buffer/dirty) is asserted by any authority",
    "context": "weaver inspect 1:buffer/dirty",
    "fact_key": { "entity": 1, "attribute": "buffer/dirty" }
  }
}
```

## Binary: `weaver-tui`

### Subcommands

| Subcommand | Purpose |
|---|---|
| `weaver-tui` (no subcommand) | Connect to the core's bus, render dirty facts, accept in-TUI commands |
| `weaver-tui --version` | Print build provenance (same shape as `weaver --version`) |

### Global flags

```
    --socket=<path>      override the default bus socket path
    --no-color           disable color output
```

### In-TUI commands

| Key | Command | Effect |
|---|---|---|
| `e` | simulate-edit | Publishes `Event { name: "buffer/edited", target: EntityRef(1) }` |
| `c` | simulate-clean | Publishes `Event { name: "buffer/cleaned", target: EntityRef(1) }` |
| `i` | inspect | Issues `InspectRequest` for the currently-displayed dirty fact; renders the response |
| `q` | quit | Disconnect and exit |

### TUI render shape

```
┌─ Weaver TUI ────────────────────────────────────────────────────┐
│ Connection: ready (bus v0.1.0, core 0.1.0+a1b2c3d)              │
│                                                                 │
│ Facts:                                                          │
│   buffer/dirty(EntityRef(1)) = true                             │
│     by core::dirty_tracking, event 42, 0.142s ago               │
│                                                                 │
│ Commands: [e]dit  [c]lean  [i]nspect  [q]uit                    │
└─────────────────────────────────────────────────────────────────┘
```

When the core is unreachable:
```
┌─ Weaver TUI ────────────────────────────────────────────────────┐
│ Connection: UNAVAILABLE                                         │
│   reason: core not reachable at /run/user/1000/weaver.sock      │
│   facts shown below are the last-known view (may be stale)      │
│                                                                 │
│ Facts (stale): (none)                                           │
│                                                                 │
│ Commands: [r]econnect  [q]uit                                   │
└─────────────────────────────────────────────────────────────────┘
```

## Configuration schema

Both binaries respect `$XDG_CONFIG_HOME/weaver/config.toml` (falls back to `~/.config/weaver/config.toml`):

```toml
# All keys optional; defaults shown.
socket_path = "$XDG_RUNTIME_DIR/weaver.sock"
log_level = "info"      # one of: error, warn, info, debug, trace
```

Environment variables:

| Variable | Effect |
|---|---|
| `WEAVER_SOCKET` | Overrides `socket_path` |
| `RUST_LOG` | Overrides `log_level` (per `tracing-subscriber` `EnvFilter`) |

CLI flags override env vars override config file.

## Versioning policy

CLI surfaces are public per L2 P7. Compatibility regime:

- **PATCH**: error-message wording, log-output formatting tweaks.
- **MINOR**: new subcommands, new flags, new fields in JSON output (additive only).
- **MAJOR**: removed flags, renamed subcommands, removed JSON fields, changed exit-code semantics.

`CHANGELOG.md` records every change to the CLI surface per L2 P8.
