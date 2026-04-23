//! Per-buffer file observation: streams the on-disk content through a
//! SHA-256 hasher, compares against the in-memory digest cached in the
//! [`crate::model::BufferState`], and returns a typed
//! [`crate::model::BufferObservation`] describing byte size + dirty
//! flag + observability.
//!
//! TODO: slice 003 — `observe_buffer()` and its `ObserverError`
//! branches land here in Phase 3 (tasks T026 + T027) per
//! `specs/003-buffer-service/tasks.md`.
