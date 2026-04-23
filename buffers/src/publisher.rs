//! Bus client: publishes `buffer/*` and `watcher/status` facts, owns
//! the per-buffer poll loop, manages the per-invocation
//! `ActorIdentity::Service` identity, and handles clean-shutdown
//! retraction and bus-EOF exit paths.
//!
//! TODO: slice 003 — `run()`, `reader_loop()`, bootstrap +
//! edge-triggered dirty/observable publishing, service-level
//! `watcher/status` lifecycle, and shutdown-retract land here in
//! Phase 3 (tasks T028 – T038) per
//! `specs/003-buffer-service/tasks.md`.
