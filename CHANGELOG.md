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
