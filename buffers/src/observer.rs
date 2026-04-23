//! Per-buffer file observation: streams the on-disk content through a
//! SHA-256 hasher, compares against the in-memory digest cached in the
//! [`crate::model::BufferState`], and returns a typed
//! [`crate::model::BufferObservation`] describing byte size + dirty
//! flag + observability.
//!
//! The function is pure in the sense that it performs no bus I/O and
//! does not mutate the `BufferState` — the publisher's poll loop owns
//! the edge-triggered state transitions (tasks T034 / T035 in Phase 3).

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::model::{BufferObservation, BufferState, ObserverError};

/// Streaming chunk size for the on-disk read. 8 KiB keeps per-poll
/// allocation bounded regardless of file size; matches BufReader's
/// default and satisfies the research §2 strategy of streaming
/// through the hasher rather than materialising a second copy.
const READ_CHUNK: usize = 8 * 1024;

/// Observe a buffer's on-disk state relative to its in-memory state.
///
/// Opens the backing file fresh, streams it through a SHA-256 hasher,
/// and compares the disk digest to the `state.memory_digest()` cached
/// at open time. Returns a [`BufferObservation`] on success or a
/// categorised [`ObserverError`] on failure (see below).
///
/// `byte_size` in the returned observation is the *in-memory* content
/// size — constant throughout slice 003 (no mutation path). Slice 004+
/// keeps it in lock-step with the service's memory byte store.
///
/// # Error categorisation
///
/// - [`ObserverError::Missing`] — the path no longer exists (e.g.,
///   deleted or unmounted). Maps from `ErrorKind::NotFound`.
/// - [`ObserverError::NotRegularFile`] — the path exists but is no
///   longer a regular file (replaced by a directory, socket, symlink
///   to non-regular, etc.).
/// - [`ObserverError::TransientRead`] — any other I/O error during
///   metadata lookup, open, or read (permission flicker, mid-rename
///   race, short-read errors).
///
/// Startup failures are reported by [`BufferState::open`] as
/// [`ObserverError::StartupFailure`]; this path is not exercised by
/// the poll-loop observer.
pub fn observe_buffer(state: &BufferState) -> Result<BufferObservation, ObserverError> {
    let path = state.path();
    let metadata = std::fs::metadata(path).map_err(|source| classify_io_err(source, path))?;
    if !metadata.file_type().is_file() {
        return Err(ObserverError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }

    let file = std::fs::File::open(path).map_err(|source| classify_io_err(source, path))?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; READ_CHUNK];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|source| ObserverError::TransientRead {
                path: path.to_path_buf(),
                source,
            })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let disk_digest: [u8; 32] = hasher.finalize().into();

    Ok(BufferObservation {
        byte_size: state.byte_size(),
        dirty: &disk_digest != state.memory_digest(),
        observable: true,
    })
}

/// Classify an `io::Error` from `metadata()` or `File::open()` into an
/// [`ObserverError`]. `NotFound` maps to [`ObserverError::Missing`];
/// every other kind (permission flicker, I/O error, etc.) maps to
/// [`ObserverError::TransientRead`]. The caller's NotRegularFile path
/// is handled separately — it's derived from a *successful* metadata
/// lookup whose `file_type()` says otherwise, not from an error kind.
fn classify_io_err(err: std::io::Error, path: &Path) -> ObserverError {
    if err.kind() == std::io::ErrorKind::NotFound {
        ObserverError::Missing {
            path: path.to_path_buf(),
        }
    } else {
        ObserverError::TransientRead {
            path: path.to_path_buf(),
            source: err,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write as _;
    use std::path::PathBuf;

    use tempfile::{NamedTempFile, TempDir};

    /// Create a regular-file fixture with `content`, canonicalise, and
    /// open a `BufferState` over it. Returns the state, the guard for
    /// the backing file, and its canonical path. The caller holds the
    /// guard to control lifetime (so we can simulate disappearance).
    fn open_state(content: &[u8]) -> (BufferState, NamedTempFile, PathBuf) {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(content).expect("write fixture");
        let canonical = std::fs::canonicalize(f.path()).expect("canonicalize fixture path");
        let state = BufferState::open(canonical.clone()).expect("open fixture");
        (state, f, canonical)
    }

    #[test]
    fn observe_clean_when_disk_matches_memory() {
        let content = b"hello buffer\n";
        let (state, _guard, _) = open_state(content);
        let obs = observe_buffer(&state).expect("observe");
        assert_eq!(obs.byte_size, content.len() as u64);
        assert!(!obs.dirty, "clean: disk digest must match memory digest");
        assert!(obs.observable);
    }

    #[test]
    fn observe_dirty_when_disk_drifts_from_memory() {
        let (state, _guard, path) = open_state(b"initial\n");
        std::fs::write(&path, b"mutated\n").expect("mutate on disk");
        let obs = observe_buffer(&state).expect("observe");
        assert!(obs.dirty, "disk drift must flip dirty");
        assert!(obs.observable);
        // Slice 003 has no in-memory mutation path; byte_size reports
        // the state's stable in-memory size even when disk diverged.
        assert_eq!(obs.byte_size, b"initial\n".len() as u64);
    }

    #[test]
    fn observe_missing_when_file_deleted_mid_session() {
        let (state, guard, path) = open_state(b"present\n");
        drop(guard); // NamedTempFile removes the backing file on drop.
        assert!(!path.exists(), "precondition: fixture removed from disk");

        let err = observe_buffer(&state).expect_err("missing file must error");
        match err {
            ObserverError::Missing { path: p } => assert_eq!(p, path),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn observe_not_regular_file_when_path_replaced_by_directory() {
        let dir = TempDir::new().expect("tempdir");
        let target = dir.path().join("shadow");
        std::fs::write(&target, b"payload").expect("write fixture");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");

        let state = BufferState::open(canonical.clone()).expect("open regular file");

        // Replace the regular file with a directory at the same path.
        std::fs::remove_file(&canonical).expect("rm fixture");
        std::fs::create_dir(&canonical).expect("mkdir at fixture path");

        let err = observe_buffer(&state).expect_err("directory must not observe");
        match err {
            ObserverError::NotRegularFile { path } => assert_eq!(path, canonical),
            other => panic!("expected NotRegularFile, got {other:?}"),
        }
    }

    #[test]
    fn classify_io_err_maps_not_found_to_missing() {
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        let path = Path::new("/nonexistent/weaver-observer-probe");
        match classify_io_err(err, path) {
            ObserverError::Missing { path: p } => assert_eq!(p, path),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn classify_io_err_maps_permission_denied_to_transient_read() {
        let err = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let path = Path::new("/nonexistent/weaver-observer-probe");
        match classify_io_err(err, path) {
            ObserverError::TransientRead { path: p, source } => {
                assert_eq!(p, path);
                assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
            }
            other => panic!("expected TransientRead, got {other:?}"),
        }
    }

    #[test]
    fn classify_io_err_maps_other_kinds_to_transient_read() {
        for kind in [
            std::io::ErrorKind::Other,
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::UnexpectedEof,
        ] {
            let err = std::io::Error::from(kind);
            let path = Path::new("/nonexistent/weaver-observer-probe");
            match classify_io_err(err, path) {
                ObserverError::TransientRead { .. } => {}
                other => panic!("expected TransientRead for {kind:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn observe_large_content_streams_without_buffering() {
        // 4 × READ_CHUNK + a non-aligned tail → exercises the loop's
        // chunk boundary handling. We verify correctness by comparing
        // to the in-memory digest cached by BufferState::open, which
        // computes over the full file.
        let size = READ_CHUNK * 4 + 137;
        let content: Vec<u8> = (0..size).map(|i| (i & 0xff) as u8).collect();
        let (state, _guard, _) = open_state(&content);
        let obs = observe_buffer(&state).expect("observe");
        assert!(!obs.dirty, "multi-chunk clean observation must match");
        assert_eq!(obs.byte_size, size as u64);
    }
}
