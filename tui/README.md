# `weaver-tui` — terminal UI for Weaver

The `weaver-tui` binary for the [Hello-fact slice (001)](../specs/001-hello-fact/).

## What this crate does

`weaver-tui` is a **bus client + terminal renderer**. It:

1. Connects to the core's Unix socket and completes the `Hello` →
   `Lifecycle(Ready)` handshake (via the shared `weaver_core::bus::client::Client`).
2. Subscribes to the `buffer/*` fact family.
3. Runs a `crossterm` raw-mode event loop multiplexing:
   - **keystrokes** — `e` edit, `c` clean, `i` inspect, `q` quit
   - **bus messages** — `FactAssert` / `FactRetract` / `InspectResponse`
4. Renders a small framed panel with connection status, live facts (with
   "by behavior X, event Y, Δs ago" annotations), optional inspection
   block, and the command footer.

On core disconnect (stream error or process exit), it marks facts as
stale, switches the status line to `UNAVAILABLE`, and continues to
respond to `q` for clean exit (SC-004).

## Usage

```bash
# In one terminal — start the core.
cargo run --bin weaver -- run

# In another terminal — start the TUI.
cargo run --bin weaver-tui
```

Then press `e`, `c`, or `i` in the TUI pane.

Flags:

```
    --socket=<path>      override the default bus socket path
    --no-color           disable color output
-V, --version            print build provenance
```

`$WEAVER_SOCKET` overrides the socket path.

## Render shape

```
┌─ Weaver TUI ────────────────────────────────────────────────────┐
│ Connection: ready (bus v0.1.0)                                  │
│                                                                 │
│ Facts:                                                          │
│   buffer/dirty(EntityRef(1)) = true                             │
│     by behavior:core/dirty-tracking, event N, 0.142s ago        │
│                                                                 │
│ Commands: [e]dit  [c]lean  [i]nspect  [q]uit                    │
└─────────────────────────────────────────────────────────────────┘
```

On disconnect:

```
┌─ Weaver TUI ────────────────────────────────────────────────────┐
│ Connection: UNAVAILABLE                                         │
│   reason: bus stream error: ...                                 │
│   facts shown below are the last-known view (may be stale)      │
│                                                                 │
│ Facts (stale):                                                  │
│   buffer/dirty(EntityRef(1)) = true                             │
│     last seen before disconnect                                 │
│                                                                 │
│ Commands: [q]uit                                                │
└─────────────────────────────────────────────────────────────────┘
```

## Module map

| Module | Role |
|---|---|
| `args` | `clap` derive for the `weaver-tui` binary. |
| `client` | Thin wrapper over `weaver_core::bus::client::Client`; spawns a background reader task that forwards inbound messages / disconnect signals via `mpsc`. |
| `commands` | Keystroke handlers — `publish` (edit/clean) and `inspect`. |
| `render` | Crossterm raw-mode event loop + `AppState` + `draw`. |

## Deferred

- `ratatui` migration — current render is ~30 lines of manual `crossterm` output. `ratatui` lands when render logic grows.
- Reconnect after core exit — current spec Assumptions defer this; `q` + restart is the supported flow for slice 001.
