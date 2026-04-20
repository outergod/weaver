# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/).
Per-public-surface versioning is per L2 constitution Principle 8 (`.specify/memory/constitution.md`).

## Public surfaces tracked

Per L2 Principle 7, each public surface carries its own version. Surfaces introduced in this initial release are at v0.1.0 (initial — no prior to migrate from):

- **Bus protocol** v0.1.0 — message categories, delivery classes (lossy / authoritative), CBOR tag scheme entries 1000 (entity-ref) and 1001 (keyword). See `specs/001-hello-fact/contracts/bus-messages.md`.
- **Fact-family schema `buffer/dirty`** v0.1.0 — boolean fact on a buffer entity indicating unsaved changes.
- **CLI surface** v0.1.0 — `weaver run`, `weaver --version`, `weaver status`, `weaver inspect`, `weaver simulate-edit`, `weaver simulate-clean`; `weaver-tui` binary; global flags `-v`, `-o`/`--output=<format>`, `--socket=<path>`. See `specs/001-hello-fact/contracts/cli-surfaces.md`.
- **Configuration schema** v0.1.0 — XDG-based config file with `socket_path`, `log_level` keys; env vars `WEAVER_SOCKET`, `RUST_LOG`.

## [Unreleased]

### Added — slice 001 "Hello, fact" (in progress)

Entries land per phase of `specs/001-hello-fact/tasks.md`. They will be promoted into a dated, versioned section once the slice is complete.

#### Phase 1 — Setup

- Workspace `Cargo.toml` with `[workspace.package]` (edition 2024, rust-version 1.85, license `AGPL-3.0-or-later` matching `LICENSE`) and `[workspace.dependencies]` for tokio, serde + serde_json, ciborium, clap, miette + thiserror, tracing, proptest, vergen, crossterm. The initial scaffold incorrectly defaulted the license to `MIT OR Apache-2.0` (Rust-ecosystem default); aligned to AGPL per L2 Amendment 4.
- `rust-toolchain.toml` pinning the stable Rust channel for reproducible builds (L2 P19).
- `.gitignore` Rust patterns (`target/`, `**/*.rs.bk`, `*.sock`) with explicit guidance to keep `Cargo.lock` tracked (L2 P19).
- `core` crate scaffold with `[lib]` (`weaver_core`) and `[[bin]]` (`weaver`) targets and `build.rs` invoking `vergen` for build-time provenance (L2 P11).
- `tui` crate scaffold (`weaver-tui` binary) depending on `weaver_core` for shared types.
- `ui` crate stub (Tauri UI deferred per Hello-fact slice 001 scope).

#### Phase 3 — User Story 1 (MVP: trigger + propagate)

- **Fact-family `buffer/dirty` (v0.1.0)**: first live producer. `core/dirty-tracking` behavior asserts `buffer/dirty=true` on `buffer/edited` events and retracts it on `buffer/cleaned`.
- **Bus protocol (v0.1.0)**: subscriptions now *forward* `FactAssert`/`FactRetract` messages to subscribers in real time. The Phase 2 listener acked subscriptions but never forwarded; the new listener multiplexes client reads and subscription fan-out via `tokio::select!`. No wire-format change — the behavior completes what v0.1.0 always promised.
- **CLI surface (v0.1.0)**:
  - `weaver simulate-edit <buffer-id>` now publishes `buffer/edited` on the bus (previous Phase 2 stub was a warn log).
  - `weaver simulate-clean <buffer-id>` now publishes `buffer/cleaned` (previous Phase 2 stub).
  - Both commands return a structured submission ack in `--output=human` or `--output=json`.
- **TUI**: crossterm raw-mode event loop with `e`/`c`/`q` keystrokes; live rendering of subscribed facts with `by <behavior>, event <id>, Δs ago` annotation; stale-view rendering with `UNAVAILABLE` status on core disconnect per `contracts/cli-surfaces.md`.
- **Dispatcher**: commit is now atomic with respect to behavior error — when a behavior firing returns `error: Some(_)`, its assertions and retractions are rolled back and the `BehaviorFired` trace entry records empty `asserted`/`retracted` lists. Tightens the implicit contract that Phase 2's docstring already claimed; covered by the new `error_recovery` scenario test.
- **Shared bus-client helper (`core/src/bus/client.rs`)**: consolidates the `Hello`/`Lifecycle(Ready)`/`Subscribe` handshake used by the CLI's one-shot subcommands, the TUI, and the e2e harness. Consolidation paves the way for the inspect client in Phase 4.
- **Workspace member `weaver-e2e`** (`tests/`): workspace-level end-to-end tests spawning the `weaver` binary. Two tests ship with this phase: `hello_fact` (SC-001, happy + retraction round-trip ≤ 100 ms) and `disconnect` (SC-004, SIGKILL survivability within 5 s).

#### Phase 4 — User Story 2 (provenance inspection)

- **Bus protocol (v0.1.0)**: `InspectRequest` now returns a real `InspectionDetail` instead of an always-`FactNotFound` placeholder. The handler walks the trace store's reverse causal index (already built in Phase 2) and is `O(1)` per lookup.
- **CLI surface (v0.1.0)**:
  - `weaver inspect <entity-id>:<attribute>` is now live. Parses the colon-delimited fact key, issues `InspectRequest`, renders human or JSON output matching `contracts/cli-surfaces.md`. Exit code 2 on `FactNotFound`.
  - Input validation — malformed keys (missing colon, empty halves, non-numeric entity id) produce structured errors before touching the bus.
- **TUI**: `i` keystroke triggers inspection of the first displayed fact. Waiting state rendered explicitly (`(waiting for response…)`) between request send and response; correlation via `request_id` so out-of-order InspectResponses are handled safely.
- **`core/src/inspect/handler.rs`** (new): pure routine `inspect_fact(snapshot, trace, key) -> Result<InspectionDetail, InspectionError>`. Uses `FactSpaceSnapshot` for the current-assertion check (fast `Arc` clone) and `TraceStore::fact_inspection` for the asserting behavior/event lookup.
- **New test coverage**: `inspect_inspection_found`, `inspect_inspection_not_found`, `property_inspection_invariant`, plus fact-key-parser unit tests in `cli::inspect::tests`.

#### Phase 5 — User Story 3 (structured machine output)

- **Bus protocol (v0.1.0 — additive)**: two new `BusMessage` variants — `StatusRequest` (client → core, unit) and `StatusResponse { lifecycle, uptime_ns, facts }` (core → client). Additive surface change per L2 P7. A future slice with a deployed v0.1 client will bump `Hello.protocol_version` if a wire-incompatible change ships; the protocol-level CBOR deserializer does NOT yet handle unknown variants gracefully, so adding variants *today* is only safe because all clients are co-developed in this repo. This caveat is a known gap in the contract — to be tightened in a future slice.
- **CLI surface (v0.1.0)**:
  - `weaver status [-o human|json]` is now live (was a warn-log stub). Connects to the bus, sends `StatusRequest`, renders the response per `contracts/cli-surfaces.md`.
  - On `core-unavailable`: renders the documented `{"lifecycle": "unavailable", "error": "..."}` shape and exits `2`.
  - Exit-code policy centralised in `cli::errors::exit_code` (`OK=0`, `GENERAL=1`, `EXPECTED=2`).
- **Error surface (new)**: `WeaverCliError` in `core/src/cli/errors.rs` with `miette::Diagnostic` derive. Four codes wired up (`WEAVER-002` core-unavailable, `WEAVER-101` parse-error, `WEAVER-201` fact-not-found, `WEAVER-301` protocol-error). JSON envelope matches contract: `{ "error": { "category", "code", "message", "context", "fact_key" } }`. `fact_key` populated for fact-scoped errors per L2 P6.
- **Dispatcher**: tracks `started_at_ns`; exposes `Dispatcher::uptime_ns()` for the status handler.
- **Listener**: handles `StatusRequest` by snapshotting the fact-store, reading dispatcher uptime, and replying with `StatusResponse`.
- **`core/src/cli/output.rs`** (new): `StatusResponse` struct with serde round-trip, `render_status` dispatcher, human and JSON formatters. Unit tests verify (a) round-trip preservation, (b) ready shape omits `error`, (c) unavailable shape omits `uptime_ns` and `facts`.
- **New test coverage**: `cli_status_round_trip`, `cli_status_unavailable` (exit-code 2), `cli_status_human`, plus `cli::output::tests` (4) and `cli::errors::tests` (3).

#### Phase 5.5 — Wire-tagging alignment (pre-1.0 cleanup)

The first slice is the right time to unify serialization strategy across
public wire surfaces. Adopted **adjacent tagging** (`"type"` + variant-specific
content field) uniformly for every sum type with non-unit variants:

- **`SourceId`** — `#[serde(tag = "type", content = "id")]`. Wire form now
  matches `contracts/cli-surfaces.md`'s example literally:
  `{"type":"behavior","id":"core/dirty-tracking"}` (was `{"behavior":"..."}`).
- **`BusMessage`** — `#[serde(tag = "type", content = "payload")]`. Every
  message variant now shares the shape `{"type":"<kind>","payload":<data>}`
  (unit variants omit `payload`). Uniform `.type`-based dispatch for
  consumers (was a rotating outer key per variant).
- **`SubscribePattern`** — `#[serde(tag = "type", content = "pattern")]`.
  `{"type":"family-prefix","pattern":"buffer/"}` (was `{"family-prefix":"..."}`).

`FactValue` already used adjacent tagging (`tag="type", content="value"`) — all
four data-bearing enums now share the pattern. Unit-only enums
(`LifecycleSignal`, `EventPayload`, `InspectionError`) remain bare kebab-case
strings, which is naturally consistent with adjacent-tag content semantics.

**Why now and not later**: the bus protocol had no deployed external
consumers. The change is wire-breaking for any already-serialized CBOR
payload (none existed outside this repo). Every round-trip test passed
without modification — serde handles both encode and decode through the
same Rust types, so the proof that clients still agree with the core
runs as part of `cargo test`.

`contracts/bus-messages.md` now documents the tagging convention as a
first-class principle.
