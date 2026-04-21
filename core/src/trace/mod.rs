//! Append-only trace store with reverse causal index.
//!
//! Snapshot-and-truncate retention is the architectural commitment per
//! `docs/02-architecture.md` §10.2. Slice 001 runs in-memory only; a
//! persistent trace store is a later milestone.

pub mod entry;
pub mod store;
