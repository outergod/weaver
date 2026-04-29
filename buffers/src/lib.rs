//! `weaver-buffers` library — slice 003 Phase 1 scaffold.
//!
//! The buffer service is Weaver's first content-backed service: it
//! opens one or more files named on the CLI, holds a `:content`
//! component per opened file (conceptual — no in-code `Component`
//! type yet; see `docs/07-open-questions.md §26`), and publishes a
//! small set of *derived* facts over the bus (`buffer/path`,
//! `buffer/byte-size`, `buffer/dirty`, `buffer/observable`) under an
//! `ActorIdentity::Service`.
//!
//! Module map (populated across slice 003's phases):
//!
//! - `model`     — [`BufferState`], entity-id derivation, observation types.
//! - `observer`  — per-poll file read + SHA-256 digest + dirty check.
//! - `publisher` — bus client: handshake, per-buffer bootstrap, poll
//!   loop, shutdown-retract.
//! - `cli`       — clap-derive CLI entry point, miette diagnostics,
//!   exit-code mapping.
//! - `version`   — contracted `--version` rendering (human + JSON).

pub(crate) mod atomic_write;
pub mod cli;
pub mod model;
pub mod observer;
pub mod publisher;
pub mod version;
