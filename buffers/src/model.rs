//! Buffer-service domain types.
//!
//! Contents:
//!
//! - [`buffer_entity_ref`] / [`watcher_instance_entity_ref`] —
//!   entity-id derivation with reserved namespace bits.
//! - [`BufferState`] — in-memory state for one opened buffer, with a
//!   structurally enforced `memory_digest == sha256(content)` invariant.
//! - [`BufferObservation`] — pure output of one observation tick.
//! - [`ObserverError`] — categorised failure modes for
//!   [`BufferState::open`] and the observer path.
//!
//! See `specs/003-buffer-service/data-model.md`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;
use weaver_core::types::entity_ref::EntityRef;

/// Buffer-namespace bit in the derived `EntityRef`. Set on every
/// buffer entity; distinct from the slice-002 reserved bits 62
/// (watcher-instance) and 63 (repo). Trace inspection can classify an
/// entity at a glance by this bit.
const BUFFER_NAMESPACE_BIT: u64 = 1 << 61;

/// Watcher-instance-namespace bit. Reused unchanged from slice 002: a
/// `weaver-buffers` invocation's instance entity shares the namespace
/// with git-watcher instances — the TUI/inspect machinery distinguishes
/// them by asserted facts, not by entity-id bit layout.
const INSTANCE_NAMESPACE_BIT: u64 = 1 << 62;

/// Repo-namespace bit, owned by slice-002's `git-watcher`. Buffer
/// derivations clear this bit so a low-order hash never accidentally
/// claims the repo namespace.
const REPO_NAMESPACE_BIT: u64 = 1 << 63;

/// Derive a stable `EntityRef` for a buffer entity from its
/// canonicalized absolute path.
///
/// The caller MUST pass an already-canonicalized path (e.g., the output
/// of [`std::fs::canonicalize`]). This function is a pure function of
/// its input — no I/O, no implicit canonicalization — so the entity id
/// is deterministic for any canonical path.
pub fn buffer_entity_ref(path: &Path) -> EntityRef {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let h =
        (hasher.finish() | BUFFER_NAMESPACE_BIT) & !(INSTANCE_NAMESPACE_BIT | REPO_NAMESPACE_BIT);
    EntityRef::new(h)
}

/// Derive a stable `EntityRef` for the buffer-service-instance entity
/// (host of `watcher/status` for this invocation). Mirrors slice-002's
/// watcher-instance derivation; bit 62 set, bit 63 cleared.
pub fn watcher_instance_entity_ref(instance: &Uuid) -> EntityRef {
    let mut hasher = DefaultHasher::new();
    instance.as_bytes().hash(&mut hasher);
    let h = (hasher.finish() | INSTANCE_NAMESPACE_BIT) & !REPO_NAMESPACE_BIT;
    EntityRef::new(h)
}

/// In-memory state for a single opened buffer.
///
/// Invariant: `memory_digest == Sha256(content)` at all times. Enforced
/// structurally via private fields plus the fallible [`Self::open`]
/// constructor; slice 003 has no in-process mutation path. Slice 004+
/// will expose a `set_content` that updates both fields together.
///
/// Custom [`std::fmt::Debug`] redacts `content` so accidental
/// `tracing::debug!(?state)` never emits the bytes of an opened file —
/// the slice's defining invariant (FR-002a) is that content must never
/// leak through any API, including debug formatting.
pub struct BufferState {
    path: PathBuf,
    entity: EntityRef,
    content: Vec<u8>,
    memory_digest: [u8; 32],
    last_dirty: bool,
    last_observable: bool,
}

impl std::fmt::Debug for BufferState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferState")
            .field("path", &self.path)
            .field("entity", &self.entity)
            .field("byte_size", &self.byte_size())
            .field("memory_digest", &hex_digest(&self.memory_digest))
            .field("last_dirty", &self.last_dirty)
            .field("last_observable", &self.last_observable)
            .finish_non_exhaustive()
    }
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl BufferState {
    /// Open `path` and construct a fresh `BufferState`.
    ///
    /// Reads the file's content into memory, computes its SHA-256
    /// digest, and initialises `last_dirty=false` / `last_observable=true`.
    /// `path` MUST already be canonicalized.
    ///
    /// Returns [`ObserverError::StartupFailure`] if the file is missing,
    /// is a directory, is unreadable, or cannot be loaded into memory;
    /// the variant's `kind` selects the CLI diagnostic code
    /// (WEAVER-BUF-001..003).
    pub fn open(path: PathBuf) -> Result<Self, ObserverError> {
        let metadata = std::fs::metadata(&path).map_err(|source| {
            // Every metadata failure (NotFound, PermissionDenied, I/O)
            // renders under WEAVER-BUF-001. NotRegularFile is checked
            // against a *successful* metadata below; too-large is
            // checked against the read result.
            ObserverError::StartupFailure {
                path: path.clone(),
                reason: source.to_string(),
                kind: StartupKind::NotOpenable,
            }
        })?;
        if !metadata.file_type().is_file() {
            return Err(ObserverError::StartupFailure {
                path: path.clone(),
                reason: "path is not a regular file (directory, symlink, or special file)".into(),
                kind: StartupKind::NotRegularFile,
            });
        }
        let content = match std::fs::read(&path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::OutOfMemory => {
                return Err(ObserverError::StartupFailure {
                    path: path.clone(),
                    reason: e.to_string(),
                    kind: StartupKind::TooLarge,
                });
            }
            Err(e) => {
                return Err(ObserverError::StartupFailure {
                    path: path.clone(),
                    reason: e.to_string(),
                    kind: StartupKind::NotOpenable,
                });
            }
        };
        let memory_digest: [u8; 32] = Sha256::digest(&content).into();
        let entity = buffer_entity_ref(&path);
        Ok(Self {
            path,
            entity,
            content,
            memory_digest,
            last_dirty: false,
            last_observable: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entity(&self) -> EntityRef {
        self.entity
    }

    pub fn content(&self) -> &[u8] {
        &self.content
    }

    pub fn memory_digest(&self) -> &[u8; 32] {
        &self.memory_digest
    }

    pub fn byte_size(&self) -> u64 {
        self.content.len() as u64
    }

    pub fn last_dirty(&self) -> bool {
        self.last_dirty
    }

    pub fn last_observable(&self) -> bool {
        self.last_observable
    }

    /// Record the publisher's most-recently-asserted `buffer/dirty`
    /// value for this buffer, so the next poll tick can edge-trigger
    /// the transition (re-publish only when the flag flips).
    pub(crate) fn set_last_dirty(&mut self, v: bool) {
        self.last_dirty = v;
    }

    /// Record the publisher's most-recently-asserted `buffer/observable`
    /// value — the per-buffer complement of `last_dirty`'s edge rule.
    pub(crate) fn set_last_observable(&mut self, v: bool) {
        self.last_observable = v;
    }
}

/// Pure output of one observation tick — what the publisher needs to
/// decide which facts to re-assert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferObservation {
    pub byte_size: u64,
    pub dirty: bool,
    pub observable: bool,
}

/// Classification of [`ObserverError::StartupFailure`]. Drives the
/// CLI's `miette::Diagnostic` rendering by selecting the
/// `WEAVER-BUF-00{1,2,3}` code per `contracts/cli-surfaces.md §Error
/// rendering`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupKind {
    /// WEAVER-BUF-001 — generic open failure: missing, permission
    /// denied, I/O error. The most common startup failure.
    NotOpenable,
    /// WEAVER-BUF-002 — path exists but is a directory, symlink to a
    /// non-regular target, socket, etc.
    NotRegularFile,
    /// WEAVER-BUF-003 — file exceeded available memory at open time.
    /// Reserved for a later-slice configurable size limit; currently
    /// triggered only if [`std::io::ErrorKind::OutOfMemory`]
    /// propagates from `std::fs::read`.
    TooLarge,
}

/// Errors produced by [`BufferState::open`] and the observer path.
#[derive(Debug, Error)]
pub enum ObserverError {
    /// File not readable mid-session (transient: permission flicker,
    /// mid-rename race). Publisher flips `buffer/observable=false`.
    #[error("transient read error for {path}: {source}")]
    TransientRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// File does not exist mid-session (deleted, unmounted). Publisher
    /// flips `buffer/observable=false`.
    #[error("buffer file missing: {path}")]
    Missing { path: PathBuf },

    /// File exists but is no longer a regular file (replaced by a
    /// directory, socket, etc.). Publisher flips
    /// `buffer/observable=false`; the buffer stays tracked but is lost.
    #[error("buffer path is no longer a regular file: {path}")]
    NotRegularFile { path: PathBuf },

    /// Startup-only: path invalid at open time. Publisher exits code 1
    /// after the CLI layer emits a WEAVER-BUF-00{1,2,3} diagnostic
    /// chosen by `kind`.
    #[error("buffer not openable at {path}: {reason}")]
    StartupFailure {
        path: PathBuf,
        reason: String,
        kind: StartupKind,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn path(bytes: &str) -> PathBuf {
        PathBuf::from(bytes)
    }

    #[test]
    fn buffer_entity_reserved_bits_set_correctly() {
        for p in [
            "/tmp/a",
            "/home/alex/code/weaver/core/src/lib.rs",
            "/",
            "/this/is/a/very/long/canonical/path/that/exercises/the/hasher",
        ] {
            let e = buffer_entity_ref(&path(p)).as_u64();
            assert!(e & BUFFER_NAMESPACE_BIT != 0, "bit 61 must be set for {p}");
            assert!(
                e & INSTANCE_NAMESPACE_BIT == 0,
                "bit 62 must be clear for {p}"
            );
            assert!(e & REPO_NAMESPACE_BIT == 0, "bit 63 must be clear for {p}");
        }
    }

    #[test]
    fn buffer_entity_is_deterministic() {
        let p = path("/home/alex/file.txt");
        assert_eq!(buffer_entity_ref(&p), buffer_entity_ref(&p));
    }

    #[test]
    fn buffer_entity_distinguishes_paths() {
        let a = buffer_entity_ref(&path("/tmp/a"));
        let b = buffer_entity_ref(&path("/tmp/b"));
        assert_ne!(a, b);
    }

    #[test]
    fn watcher_instance_entity_reserved_bits_set_correctly() {
        for _ in 0..8 {
            let id = Uuid::new_v4();
            let e = watcher_instance_entity_ref(&id).as_u64();
            assert!(e & INSTANCE_NAMESPACE_BIT != 0, "bit 62 must be set");
            assert!(e & REPO_NAMESPACE_BIT == 0, "bit 63 must be clear");
        }
    }

    #[test]
    fn watcher_instance_entity_is_deterministic() {
        let id = Uuid::new_v4();
        assert_eq!(
            watcher_instance_entity_ref(&id),
            watcher_instance_entity_ref(&id)
        );
    }

    #[test]
    fn watcher_instance_entity_distinguishes_uuids() {
        let a = watcher_instance_entity_ref(&Uuid::new_v4());
        let b = watcher_instance_entity_ref(&Uuid::new_v4());
        assert_ne!(a, b, "two random uuids must produce distinct entities");
    }

    #[test]
    fn buffer_state_open_establishes_digest_invariant() {
        let mut f = NamedTempFile::new().expect("tempfile");
        let content = b"hello buffer\n";
        f.write_all(content).expect("write");
        let canonical = std::fs::canonicalize(f.path()).expect("canonicalize");

        let state = BufferState::open(canonical.clone()).expect("open");
        assert_eq!(state.path(), canonical.as_path());
        assert_eq!(state.entity(), buffer_entity_ref(&canonical));
        assert_eq!(state.content(), content);
        assert_eq!(state.byte_size(), content.len() as u64);

        // Invariant: memory_digest == sha256(content).
        let expected: [u8; 32] = Sha256::digest(content).into();
        assert_eq!(state.memory_digest(), &expected);

        // Initial transient flags.
        assert!(!state.last_dirty(), "initial dirty must be false");
        assert!(state.last_observable(), "initial observable must be true");
    }

    #[test]
    fn buffer_state_open_missing_returns_startup_failure_not_openable() {
        let missing = path("/definitely/not/a/real/path/weaver-test-absent");
        let err = BufferState::open(missing.clone()).expect_err("missing path must fail");
        match err {
            ObserverError::StartupFailure { path, kind: k, .. } => {
                assert_eq!(path, missing);
                assert_eq!(k, StartupKind::NotOpenable);
            }
            other => panic!("expected StartupFailure, got {other:?}"),
        }
    }

    #[test]
    fn buffer_state_open_directory_returns_startup_failure_not_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = std::fs::canonicalize(dir.path()).expect("canonicalize");
        let err = BufferState::open(canonical.clone()).expect_err("directory must fail");
        match err {
            ObserverError::StartupFailure { kind: k, .. } => {
                assert_eq!(k, StartupKind::NotRegularFile);
            }
            other => panic!("expected StartupFailure::NotRegularFile, got {other:?}"),
        }
    }

    #[test]
    fn observer_error_variants_are_distinct() {
        let p = path("/tmp/x");
        let transient = ObserverError::TransientRead {
            path: p.clone(),
            source: std::io::Error::other("transient"),
        };
        let missing = ObserverError::Missing { path: p.clone() };
        let not_regular = ObserverError::NotRegularFile { path: p.clone() };
        let startup = ObserverError::StartupFailure {
            path: p,
            reason: "fixture".into(),
            kind: StartupKind::NotOpenable,
        };
        // Spot-check the Display impls so downstream diagnostics don't
        // collapse into ambiguous wording.
        assert!(format!("{transient}").contains("transient read"));
        assert!(format!("{missing}").contains("missing"));
        assert!(format!("{not_regular}").contains("regular file"));
        assert!(format!("{startup}").contains("not openable"));
    }
}
