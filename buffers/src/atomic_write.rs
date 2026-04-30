//! Atomic disk-write helper used by [`crate::model::BufferState`]'s
//! save path.
//!
//! Performs a five-step POSIX atomic write:
//!
//! 1. Open a tempfile in the same directory as the target with name
//!    `.<basename>.weaver-save.<uuid-v4-simple>`.
//! 2. Write `contents` to the tempfile.
//! 3. `fsync(2)` the tempfile.
//! 4. `rename(2)` the tempfile to the target path.
//! 5. `fsync(2)` the parent directory.
//!
//! Each step calls `before(step)` first; if the hook returns `Err`,
//! the syscall is skipped and the helper short-circuits with the
//! hook's error wrapped against that step. Tempfile cleanup is
//! attempted best-effort on any failure after step 1; after a
//! successful rename the tempfile path no longer exists and the
//! cleanup attempt's ENOENT is silently absorbed.
//!
//! Production callers pass `|_| Ok(())` (inlines to a constant);
//! tests inject errors at chosen `WriteStep` values to verify the
//! atomic-rename invariant (SC-504).
//!
//! See `specs/005-buffer-save/research.md §3, §6, §7`.

use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

#[cfg(test)]
use std::io::Error as IoError;

use uuid::Uuid;

/// One step of the atomic-write sequence. The hook surface for
/// [`atomic_write_with_hooks`] is parametric over this enum so tests
/// can target a chosen failure point.
///
/// Visibility note: `pub` only so it can be re-exported via the
/// [`crate::test_support`] module (which is `#[doc(hidden)]`) for
/// cross-crate integration tests. Production callers reach this
/// enum through the `pub(crate)` `atomic_write_with_hooks` API.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStep {
    OpenTempfile,
    WriteContents,
    FsyncTempfile,
    RenameToTarget,
    FsyncParentDir,
}

/// Atomically replace `path` with `contents`. Production callers pass
/// `|_| Ok(())` for `before`; tests inject errors per [`WriteStep`].
///
/// On `Ok(())` the target file is byte-identical to `contents` and
/// the parent directory has been `fsync`ed (durability invariant).
///
/// On `Err((step, error))` from any step before the successful
/// rename, the target file is byte-identical to its pre-call state
/// (atomic-rename invariant SC-504) and the tempfile has been removed
/// best-effort. A failure at [`WriteStep::FsyncParentDir`] means the
/// rename already succeeded — the target carries the new content but
/// durability across crash is not guaranteed.
pub(crate) fn atomic_write_with_hooks<F>(
    path: &Path,
    contents: &[u8],
    mut before: F,
) -> Result<(), (WriteStep, io::Error)>
where
    F: FnMut(WriteStep) -> Result<(), io::Error>,
{
    let parent = path
        .parent()
        .expect("atomic_write_with_hooks: target path has no parent");
    let basename = path
        .file_name()
        .expect("atomic_write_with_hooks: target path has no file_name");

    // Tempfile naming per research.md §6: dot-prefix hides from
    // default `ls`; `.weaver-save.` infix marks orphan origin; 32-char
    // UUIDv4 suffix gives 122 bits of entropy — collision-free under
    // any plausible concurrency.
    let tempfile_name = format!(
        ".{}.weaver-save.{}",
        basename.to_string_lossy(),
        Uuid::new_v4().simple()
    );
    let tempfile_path = parent.join(&tempfile_name);

    // Capture the target's mode bits so the rename preserves them.
    // Without this, an executable file (e.g., 0o755) would silently
    // drop its +x after `weaver save` because the tempfile was opened
    // with the process default (0o666 & umask). Codex P1 review on
    // PR #12. Mask `0o7777` retains permission + setuid/setgid/sticky
    // bits and drops the file-type / non-permission `mode_t` bits.
    //
    // Ownership (uid/gid) is NOT preserved — matches the convention
    // most editors follow (vim, emacs save-via-rename pattern); only
    // root could honour ownership preservation cross-uid, and most
    // workflows save under the current uid anyway.
    //
    // If the target does not yet exist, fall back to `0o666` (the
    // OpenOptions default; the kernel applies the process umask on
    // `open(2)`).
    let target_mode: u32 = match std::fs::metadata(path) {
        Ok(m) => m.permissions().mode() & 0o7777,
        Err(_) => 0o666,
    };

    // --- Step 1 --- OpenTempfile.
    if let Err(e) = before(WriteStep::OpenTempfile) {
        return Err((WriteStep::OpenTempfile, e));
    }
    let mut tempfile = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(target_mode)
        .open(&tempfile_path)
    {
        Ok(f) => f,
        Err(e) => return Err((WriteStep::OpenTempfile, e)),
    };
    // Tempfile is now on disk. Every error path below attempts
    // best-effort cleanup via `remove_file(&tempfile_path)`. After a
    // successful rename the path no longer exists; ENOENT from the
    // cleanup attempt is silently absorbed.

    // open(2) applies `mode & ~umask`, so a restrictive umask (e.g.
    // 0o077) would silently narrow our captured target_mode (e.g.
    // 0o755 → 0o700, dropping group + other rwx). Explicit
    // set_permissions bypasses umask filtering and enforces the
    // captured mode exactly. Codex P1 follow-up review on PR #12.
    if let Err(e) =
        std::fs::set_permissions(&tempfile_path, std::fs::Permissions::from_mode(target_mode))
    {
        let _ = std::fs::remove_file(&tempfile_path);
        return Err((WriteStep::OpenTempfile, e));
    }

    // --- Step 2 --- WriteContents.
    if let Err(e) = before(WriteStep::WriteContents) {
        let _ = std::fs::remove_file(&tempfile_path);
        return Err((WriteStep::WriteContents, e));
    }
    if let Err(e) = tempfile.write_all(contents) {
        let _ = std::fs::remove_file(&tempfile_path);
        return Err((WriteStep::WriteContents, e));
    }

    // --- Step 3 --- FsyncTempfile.
    if let Err(e) = before(WriteStep::FsyncTempfile) {
        let _ = std::fs::remove_file(&tempfile_path);
        return Err((WriteStep::FsyncTempfile, e));
    }
    if let Err(e) = tempfile.sync_all() {
        let _ = std::fs::remove_file(&tempfile_path);
        return Err((WriteStep::FsyncTempfile, e));
    }
    drop(tempfile);

    // --- Step 4 --- RenameToTarget.
    if let Err(e) = before(WriteStep::RenameToTarget) {
        let _ = std::fs::remove_file(&tempfile_path);
        return Err((WriteStep::RenameToTarget, e));
    }
    if let Err(e) = std::fs::rename(&tempfile_path, path) {
        let _ = std::fs::remove_file(&tempfile_path);
        return Err((WriteStep::RenameToTarget, e));
    }

    // --- Step 5 --- FsyncParentDir.
    // Hook + parent fsync. By here the rename has succeeded; the
    // tempfile path no longer exists. A failure of this step is a
    // durability hazard, not a content-correctness failure.
    if let Err(e) = before(WriteStep::FsyncParentDir) {
        return Err((WriteStep::FsyncParentDir, e));
    }
    let dir = std::fs::File::open(parent).map_err(|e| (WriteStep::FsyncParentDir, e))?;
    dir.sync_all().map_err(|e| (WriteStep::FsyncParentDir, e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    fn no_hook(_: WriteStep) -> Result<(), io::Error> {
        Ok(())
    }

    fn fail_at(step: WriteStep) -> impl FnMut(WriteStep) -> Result<(), io::Error> {
        move |s| {
            if s == step {
                Err(IoError::other("injected"))
            } else {
                Ok(())
            }
        }
    }

    fn read_dir_count_orphans(parent: &Path) -> usize {
        std::fs::read_dir(parent)
            .expect("read_dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".weaver-save.")
            })
            .count()
    }

    #[test]
    fn happy_path_writes_contents_and_returns_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"old").expect("seed target");

        atomic_write_with_hooks(&target, b"new content", no_hook).expect("happy path");

        assert_eq!(std::fs::read(&target).expect("read target"), b"new content");
        assert_eq!(
            read_dir_count_orphans(dir.path()),
            0,
            "no .weaver-save orphans after success"
        );
    }

    #[test]
    fn happy_path_works_when_target_does_not_yet_exist() {
        // The save path may run against a target that was deleted
        // since open (R4 returns PathMissing in that case before this
        // helper runs), but as a unit-level assertion the helper
        // itself does not require the target to pre-exist — `rename(2)`
        // creates it from the tempfile.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("fresh.txt");
        atomic_write_with_hooks(&target, b"hello", no_hook).expect("rename creates target");
        assert_eq!(std::fs::read(&target).expect("read"), b"hello");
    }

    #[test]
    fn injection_at_each_step_returns_matching_step_in_err() {
        for step in [
            WriteStep::OpenTempfile,
            WriteStep::WriteContents,
            WriteStep::FsyncTempfile,
            WriteStep::RenameToTarget,
            WriteStep::FsyncParentDir,
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("file.txt");
            std::fs::write(&target, b"original").expect("seed target");

            let err = atomic_write_with_hooks(&target, b"NEW", fail_at(step))
                .expect_err("hook injection must fail the call");
            assert_eq!(err.0, step, "err carries the failing step");
            assert_eq!(err.1.kind(), ErrorKind::Other);
        }
    }

    #[test]
    fn pre_rename_failure_preserves_target_byte_for_byte() {
        // The atomic-rename invariant (SC-504): under any failure
        // before `rename(2)` succeeds, the original disk file is
        // byte-identical to its pre-save state.
        for step in [
            WriteStep::OpenTempfile,
            WriteStep::WriteContents,
            WriteStep::FsyncTempfile,
            WriteStep::RenameToTarget,
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("file.txt");
            let original = b"original-content".as_slice();
            std::fs::write(&target, original).expect("seed target");

            let _ = atomic_write_with_hooks(&target, b"NEW content", fail_at(step))
                .expect_err("injection failure");

            assert_eq!(
                std::fs::read(&target).expect("read target after failed save"),
                original,
                "target must be byte-identical to pre-save state on {step:?} failure"
            );
        }
    }

    #[test]
    fn post_open_failure_cleans_up_tempfile() {
        // After step 1 succeeds the tempfile exists in the parent
        // directory; any failure between steps 2 and 4 (inclusive)
        // must remove the orphan via best-effort cleanup.
        for step in [
            WriteStep::WriteContents,
            WriteStep::FsyncTempfile,
            WriteStep::RenameToTarget,
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let target = dir.path().join("file.txt");
            std::fs::write(&target, b"x").expect("seed target");

            let _ = atomic_write_with_hooks(&target, b"NEW", fail_at(step))
                .expect_err("injection failure");

            assert_eq!(
                read_dir_count_orphans(dir.path()),
                0,
                ".weaver-save tempfile must be cleaned up after {step:?} failure"
            );
        }
    }

    #[test]
    fn open_tempfile_failure_leaves_no_orphan() {
        // Step 1 itself failed: the tempfile was never created.
        // Cleanup is trivially a no-op; assert the parent has no
        // .weaver-save entries.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"x").expect("seed target");

        let _ = atomic_write_with_hooks(&target, b"NEW", fail_at(WriteStep::OpenTempfile))
            .expect_err("open hook injection failure");
        assert_eq!(read_dir_count_orphans(dir.path()), 0);
    }

    #[test]
    fn save_preserves_target_mode_including_executable_bit() {
        // Codex P1 (PR #12): saving an executable must NOT silently
        // drop its +x after rename. The tempfile is opened with the
        // target's existing mode; rename preserves the source-inode's
        // metadata — including mode — so the post-save file matches
        // the pre-save permissions byte-for-byte.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("script.sh");
        std::fs::write(&target, b"#!/bin/sh\necho old\n").expect("seed target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .expect("chmod 0o755");

        atomic_write_with_hooks(&target, b"#!/bin/sh\necho new\n", no_hook).expect("save");

        let post_mode = std::fs::metadata(&target)
            .expect("stat post-save")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(
            post_mode, 0o755,
            "executable mode 0o755 must survive `weaver save` (got 0o{post_mode:o})",
        );
        assert_eq!(
            std::fs::read(&target).expect("read content"),
            b"#!/bin/sh\necho new\n",
            "content must reflect the new bytes",
        );
    }

    #[test]
    fn save_preserves_target_mode_under_restrictive_umask() {
        // Codex P1 follow-up (PR #12): a restrictive process umask
        // (e.g. 0o077) would silently narrow OpenOptions::mode's
        // intent; the kernel applies `mode & ~umask` at open(2). The
        // explicit set_permissions after open bypasses this and
        // enforces the captured target_mode exactly.
        //
        // Test sets umask=0o077, opens a 0o755 target, then runs
        // atomic_write_with_hooks; the post-save mode must be 0o755
        // (NOT 0o700 which is what umask-filtered open would yield).
        // umask is process-wide; restore it after the test to avoid
        // cross-test interference.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("script.sh");
        std::fs::write(&target, b"#!/bin/sh\necho old\n").expect("seed target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
            .expect("chmod 0o755");

        let prior_umask = unsafe { libc::umask(0o077) };
        let result = atomic_write_with_hooks(&target, b"#!/bin/sh\necho new\n", no_hook);
        // Restore umask BEFORE assertions so a panic doesn't leak it.
        unsafe { libc::umask(prior_umask) };
        result.expect("save");

        let post_mode = std::fs::metadata(&target)
            .expect("stat post-save")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(
            post_mode, 0o755,
            "0o755 must survive `weaver save` even under restrictive umask 0o077 \
             (got 0o{post_mode:o}); set_permissions bypass is the load-bearing piece",
        );
    }

    // Note on setuid/setgid/sticky preservation: the production code's
    // 0o7777 bitmask captures the high mode bits, and `OpenOptions::mode`
    // forwards them to `open(2)`. Verifying the round-trip in a unit
    // test is not portable — many distributions (and most container
    // runtimes) mount `/tmp` with `nosuid`, which strips the setuid /
    // setgid bits regardless of process behavior. The executable
    // (`0o755`) test above covers the operator-relevant rwx surface;
    // the high bits ride the same `target_mode` flow with no
    // additional code path.

    #[test]
    fn fsync_parent_failure_leaves_target_with_new_content() {
        // FsyncParentDir injection happens AFTER the rename succeeded;
        // the target carries the new content. The durability invariant
        // is at risk under crash but content is correct.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"old").expect("seed target");

        let err = atomic_write_with_hooks(&target, b"NEW", fail_at(WriteStep::FsyncParentDir))
            .expect_err("fsync-parent injection failure");
        assert_eq!(err.0, WriteStep::FsyncParentDir);
        assert_eq!(
            std::fs::read(&target).expect("read target after fsync-parent failure"),
            b"NEW",
            "target carries new content because rename already succeeded"
        );
        assert_eq!(
            read_dir_count_orphans(dir.path()),
            0,
            "tempfile path no longer exists after rename"
        );
    }
}
