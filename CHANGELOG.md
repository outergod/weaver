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
