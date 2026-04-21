//! `weaver-git-watcher` — the first non-editor service actor on the
//! Weaver bus. Observes one git repository and publishes authoritative
//! `repo/*` facts under a structured [`ActorIdentity::Service`].
//!
//! Slice 002 introduces this crate; see
//! `specs/002-git-watcher-actor/` for the full specification, plan,
//! research decisions, data model, and acceptance criteria.
//!
//! See also:
//! - Constitution §17 — Multi-Actor Coherence.
//! - `docs/01-system-model.md` §6 — Actor taxonomy.
//! - `docs/07-open-questions.md` §26 — Discriminated-union facts
//!   (naming-based stopgap for `repo/state/*` working-copy state).

#![deny(rust_2018_idioms)]

pub mod cli;
pub mod model;
pub mod observer;
pub mod publisher;
