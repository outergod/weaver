//! Test-only re-exports of internal hook seams for cross-crate
//! integration tests.
//!
//! Used by `tests/e2e/buffer_save_atomic_invariant.rs` (slice 005
//! T025) to inject I/O failures at [`WriteStep`] boundaries and verify
//! the atomic-rename invariant under simulated failure (SC-504).
//!
//! Production code MUST NOT import from this module — `#[doc(hidden)]`
//! on the parent `pub mod test_support` keeps it out of rustdoc, and
//! the `_for_test` / wrapper-style names of every symbol reinforce
//! that intent at the call site.

use std::io;
use std::path::Path;

pub use crate::atomic_write::WriteStep;
use crate::model::{BufferState, SaveOutcome};

/// Cross-crate invocation of [`BufferState::save_to_disk_with_hooks`].
///
/// `before` is called once per [`WriteStep`] before the corresponding
/// syscall; returning `Err` short-circuits the pipeline with
/// best-effort tempfile cleanup. See `data-model.md §SaveOutcome` for
/// the failure-step → outcome mapping.
pub fn save_to_disk_with_hooks<F>(state: &BufferState, path: &Path, before: F) -> SaveOutcome
where
    F: FnMut(WriteStep) -> Result<(), io::Error>,
{
    state.save_to_disk_with_hooks(path, before)
}

/// Override the in-memory content of an opened [`BufferState`] to
/// simulate an applied edit without going through `apply_edits`.
/// Tests use this to set up specific pre-save states (the buffer's
/// `content` differs from disk) for atomic-rename invariant checks.
///
/// The buffer's `entity` and `inode` (captured at open time) remain
/// unchanged; only `content` is rewritten.
pub fn set_buffer_content(state: &mut BufferState, content: Vec<u8>) {
    state.set_content_for_test(content);
}
