# `weaver_core` — Weaver core runtime

The core library and `weaver` binary for the [Hello-fact slice (001)](../specs/001-hello-fact/).

## What this crate is

`weaver_core` is the **non-UI side** of Weaver. It owns:

- the **fact space** (`fact_space::InMemoryFactStore`) — `HashMap`-backed keyed assertions with subscription channels;
- the **trace store** (`trace::store::TraceStore`) — append-only log with reverse causal indexes for `why?` traversal;
- the **behavior dispatcher** (`behavior::dispatcher::Dispatcher`) — single-mpsc event processing with atomic error rollback (P12);
- one shipped **embedded behavior** — `behavior::dirty_tracking::DirtyTrackingBehavior`;
- the **bus** — Unix-socket listener, length-prefixed CBOR codec, per-publisher sequence counter;
- the **inspection handler** — pure routine mapping `FactKey → InspectionDetail` via the trace store's reverse index;
- the **CLI** — `weaver run`, `weaver --version`, `weaver status`, `weaver inspect`, `weaver simulate-edit`, `weaver simulate-clean`.

The library target exposes the domain types so other crates (`weaver-tui`, `weaver-e2e`) deserialize bus messages without depending on the implementation modules.

## Running the core

```bash
# Start the bus listener on $XDG_RUNTIME_DIR/weaver.sock.
cargo run --bin weaver -- run

# In another terminal:
cargo run --bin weaver -- status --output=json | jq .
cargo run --bin weaver -- simulate-edit 1
cargo run --bin weaver -- inspect 1:buffer/dirty --output=json
```

See [`specs/001-hello-fact/quickstart.md`](../specs/001-hello-fact/quickstart.md) for the full walkthrough (SC-001 … SC-006).

## Module map

| Module | Role |
|---|---|
| `types::{entity_ref, ids, fact, event, message}` | Domain types — small composable structs per L2 P1. |
| `provenance` | `Provenance { source, timestamp_ns, causal_parent }` enforced everywhere per L2 P11. |
| `fact_space` | `FactStore` trait + `InMemoryFactStore` impl. ECS-library decision deferred — see `research.md` §13. |
| `bus::{codec, delivery, listener, client}` | Bus wire format + accept loop + shared client helper. |
| `behavior::{dispatcher, dirty_tracking}` | Single-consumer dispatcher; one embedded behavior. |
| `trace::{entry, store}` | Append-only trace with reverse causal indexes. |
| `inspect::handler` | `InspectRequest` resolution (FR-008). |
| `cli::{args, config, simulate, inspect, status, output, errors, version, tracing_setup}` | `clap` derive + subcommand implementations. |

## Contracts

Wire-level contracts live under `specs/001-hello-fact/contracts/`:

- [`bus-messages.md`](../specs/001-hello-fact/contracts/bus-messages.md) — CBOR wire format, delivery classes, tag registry (1000 `EntityRef`, 1001 `Keyword`), adjacent-tagging convention for sum types.
- [`cli-surfaces.md`](../specs/001-hello-fact/contracts/cli-surfaces.md) — CLI flags, structured output shapes, configuration schema.

## Testing

```bash
cargo test -p weaver_core          # unit + integration tests
bash scripts/ci.sh                  # full CI chain (clippy, fmt, build, test)
```

Every embedded behavior ships as a `(fact-space, event sequence) → deltas` scenario test (L2 P9) with property-based invariants on assert/retract round-trip, sequence monotonicity, and wire-level provenance (L2 P11).
