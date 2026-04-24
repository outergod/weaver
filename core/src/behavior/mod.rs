//! Behavior dispatcher — slice-003 removed the embedded
//! `core/dirty-tracking` behavior when `buffer/dirty` authority moved
//! to the `weaver-buffers` service.
//!
//! Single-VM, single-threaded for fact-space semantics per L2 P12 and
//! `docs/02-architecture.md` §9.4. Resource limits per §9.4.1 / L2 P3
//! land with the first runaway test (Phase 3 + later slices).

pub mod dispatcher;
