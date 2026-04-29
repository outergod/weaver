//! T025 — slice-005 SC-504 e2e: atomic-rename invariant under
//! simulated I/O failure at every [`WriteStep`].
//!
//! In-process test (no separate binaries / sockets). Drives
//! [`BufferState::save_to_disk_with_hooks`] directly via the
//! `weaver-buffers::test_support` seam; injects `io::Error` at each
//! step of the atomic-write sequence; asserts the on-disk file is
//! byte-identical to its pre-call state on every failure (no
//! corruption of the original from a partially-written tempfile or
//! a partial rename) and that no orphan `.weaver-save.<uuid>`
//! tempfiles survive.
//!
//! Three scenarios:
//!
//! 1. [`production_save_writes_new_content_to_disk`] — happy path:
//!    in-memory content overwrites disk content via the production
//!    hook (`|_| Ok(())`).
//!
//! 2. [`rename_step_failure_preserves_original_and_cleans_tempfile`]
//!    — explicit ENOSPC injection at [`WriteStep::RenameToTarget`]
//!    (the worst-case point — the tempfile is fully written but
//!    never named over the target). Pins the literal SC-504 narrative
//!    "rename failure ⇒ disk preserved + tempfile cleaned".
//!
//! 3. [`failure_at_every_writestep_outcome_and_atomicity`] —
//!    parametric over all five [`WriteStep`] variants; asserts the
//!    correct `SaveOutcome` mapping + tempfile cleanup at every step.
//!    Disk-preservation is asserted at the four PRE-rename steps
//!    (SC-504); at `FsyncParentDir`, `rename(2)` already completed so
//!    the new content is on disk and the test instead asserts that
//!    state (post-rename, durability is the only concern; the
//!    atomicity-of-update invariant was already satisfied).

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use weaver_buffers::model::{BufferState, SaveOutcome};
use weaver_buffers::test_support::{self, WriteStep};

#[test]
fn production_save_writes_new_content_to_disk() {
    let dir = tempdir("happy-path");
    let path = dir.join("file.txt");
    std::fs::write(&path, b"original").expect("seed disk");
    let canonical = std::fs::canonicalize(&path).expect("canonicalize");

    let mut state = BufferState::open(canonical.clone()).expect("open");
    test_support::set_buffer_content(&mut state, b"NEW content".to_vec());

    let outcome = state.save_to_disk(&canonical);
    assert!(
        matches!(outcome, SaveOutcome::Saved { .. }),
        "production save must succeed; got {outcome:?}",
    );

    let on_disk = std::fs::read(&canonical).expect("re-read");
    assert_eq!(
        on_disk, b"NEW content",
        "post-save disk content must equal in-memory content",
    );
    // Equivalence to "buffer/dirty would now compute false" — content
    // identity is the strongest form of the digest-based dirty check.
    assert_eq!(
        on_disk,
        state.content(),
        "buffer/dirty would compute false: disk == in-memory",
    );
    assert!(
        no_orphan_tempfiles(&canonical),
        "no .weaver-save.<uuid> tempfile should remain after a successful save",
    );
}

#[test]
fn rename_step_failure_preserves_original_and_cleans_tempfile() {
    let dir = tempdir("rename-failure");
    let path = dir.join("file.txt");
    let original_bytes: &[u8] = b"original";
    std::fs::write(&path, original_bytes).expect("seed disk");
    let canonical = std::fs::canonicalize(&path).expect("canonicalize");

    let mut state = BufferState::open(canonical.clone()).expect("open");
    test_support::set_buffer_content(&mut state, b"NEW content 2".to_vec());

    let inject_at = WriteStep::RenameToTarget;
    let outcome = test_support::save_to_disk_with_hooks(&state, &canonical, |step| {
        if step == inject_at {
            Err(io::Error::other("ENOSPC (injected)"))
        } else {
            Ok(())
        }
    });
    assert!(
        matches!(outcome, SaveOutcome::RenameIo { .. }),
        "rename-step failure must yield RenameIo; got {outcome:?}",
    );

    let on_disk = std::fs::read(&canonical).expect("re-read");
    assert_eq!(
        on_disk, original_bytes,
        "SC-504: original on-disk content must be byte-identical after rename failure",
    );
    assert!(
        no_orphan_tempfiles(&canonical),
        "tempfile cleanup must run after rename failure",
    );
}

#[test]
fn failure_at_every_writestep_outcome_and_atomicity() {
    // (step, label, expected_outcome_kind, rename_completed_before_failure)
    //
    // `rename_completed_before_failure` partitions WriteSteps into
    // pre-rename (4 steps; SC-504 atomicity holds — original disk
    // bytes preserved + original inode preserved) vs post-rename (1
    // step, FsyncParentDir; rename(2) already swapped the directory
    // entry, so disk has the NEW content + a NEW inode at the path).
    let cases = [
        (WriteStep::OpenTempfile, "OpenTempfile", "tempfile", false),
        (WriteStep::WriteContents, "WriteContents", "tempfile", false),
        (WriteStep::FsyncTempfile, "FsyncTempfile", "tempfile", false),
        (WriteStep::RenameToTarget, "RenameToTarget", "rename", false),
        (WriteStep::FsyncParentDir, "FsyncParentDir", "rename", true),
    ];

    for (step, label, expected_kind, rename_completed) in cases {
        let dir = tempdir(&format!("step-{label}"));
        let path = dir.join("file.txt");
        let original_bytes: &[u8] = b"the-original-bytes";
        std::fs::write(&path, original_bytes).expect("seed disk");
        let canonical = std::fs::canonicalize(&path).expect("canonicalize");
        let original_inode = std::fs::metadata(&canonical).expect("stat pre-save").ino();

        let mut state = BufferState::open(canonical.clone()).expect("open");
        let new_bytes = format!("would-write-this-on-{label}").into_bytes();
        test_support::set_buffer_content(&mut state, new_bytes.clone());

        let outcome = test_support::save_to_disk_with_hooks(&state, &canonical, |s| {
            if s == step {
                Err(io::Error::other("ENOSPC (injected)"))
            } else {
                Ok(())
            }
        });

        match (expected_kind, &outcome) {
            ("tempfile", SaveOutcome::TempfileIo { .. }) => {}
            ("rename", SaveOutcome::RenameIo { .. }) => {}
            _ => panic!(
                "WriteStep::{label} expected outcome kind {expected_kind:?}; got {outcome:?}",
            ),
        }

        let on_disk = std::fs::read(&canonical).expect("re-read");
        let post_inode = std::fs::metadata(&canonical).expect("stat post-save").ino();

        if rename_completed {
            // FsyncParentDir runs AFTER rename(2). The directory
            // entry has been updated atomically; a crash here loses
            // durability but not atomicity-of-update. The dispatcher
            // still maps this to RenameIo (consistent failure-class
            // surface for any post-tempfile error).
            assert_eq!(
                on_disk, new_bytes,
                "WriteStep::{label}: rename already completed; disk must reflect the new content",
            );
            assert_ne!(
                original_inode, post_inode,
                "WriteStep::{label}: rename already completed; inode must have changed",
            );
        } else {
            assert_eq!(
                on_disk, original_bytes,
                "SC-504: pre-rename failure at WriteStep::{label} MUST preserve original disk bytes",
            );
            assert_eq!(
                original_inode, post_inode,
                "SC-504: pre-rename failure at WriteStep::{label} MUST leave original inode",
            );
        }
        assert!(
            no_orphan_tempfiles(&canonical),
            "WriteStep::{label}: tempfile cleanup must run on failure",
        );
    }
}

// ───────────────────────────────────────────────────────────────────
// helpers
// ───────────────────────────────────────────────────────────────────

fn tempdir(label: &str) -> PathBuf {
    let pid = std::process::id();
    let tick = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!("weaver-save-atomic-e2e-{label}-{pid}-{tick}"));
    std::fs::create_dir_all(&p).expect("mkdir tempdir");
    p
}

/// Returns true iff the parent directory of `target` contains no
/// `.{basename}.weaver-save.<uuid>` orphan tempfiles. Production
/// `atomic_write_with_hooks` removes its tempfile both on the happy
/// path (rename consumes it) and on every failure step (best-effort
/// cleanup hook).
fn no_orphan_tempfiles(target: &Path) -> bool {
    let parent = target.parent().expect("target has parent");
    let basename = target
        .file_name()
        .expect("target has filename")
        .to_string_lossy()
        .into_owned();
    let prefix = format!(".{basename}.weaver-save.");
    let entries = std::fs::read_dir(parent).expect("read_dir parent");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) {
            return false;
        }
    }
    true
}
