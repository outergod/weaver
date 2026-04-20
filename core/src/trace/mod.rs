//! Append-only trace store with reverse causal index.
//!
//! Snapshot-and-truncate retention is the architectural commitment per
//! `docs/02-architecture.md` §10.2 (in-memory only for slice 001).
//!
//! Submodules land in T017 (entry types) and T024 (store).
