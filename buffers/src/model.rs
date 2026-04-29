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
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;
use weaver_core::types::edit::{Position, TextEdit};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::FactValue;

use crate::atomic_write::{WriteStep, atomic_write_with_hooks};

// Slice-004 lifted `buffer_entity_ref` (and the buffer-namespace bit)
// to `weaver_core` so the `weaver edit` CLI can derive the same
// entity-id without a cross-crate runtime dependency. The canonical
// implementation lives there; this module re-exports for slice-003
// callers (publisher + tests) and the watcher-instance derivation
// below, which still wants `INSTANCE_NAMESPACE_BIT` /
// `REPO_NAMESPACE_BIT` accessible by short name.
pub use weaver_core::types::buffer_entity::{
    BUFFER_NAMESPACE_BIT, INSTANCE_NAMESPACE_BIT, REPO_NAMESPACE_BIT, buffer_entity_ref,
};

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
    inode: u64,
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
            .field("inode", &self.inode)
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
        // Capture inode at open time. Used by the slice-005 save path
        // to refuse the write when the path/inode pair has changed
        // externally between open and save (atomic-replace, rename,
        // delete-then-create with a new inode); see WEAVER-SAVE-005.
        let inode = metadata.ino();
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
            inode,
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

    /// Inode captured at [`Self::open`] time. Pre-rename inode equality
    /// gates `WEAVER-SAVE-005` refusal in slice 005.
    pub fn inode(&self) -> u64 {
        self.inode
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

    /// Test-only: overwrite the in-memory content. Reachable across
    /// crates via [`crate::test_support::set_buffer_content`]. Used by
    /// `tests/e2e/buffer_save_atomic_invariant.rs` (slice 005 T025)
    /// to set up "edit applied" pre-save states without going through
    /// `apply_edits` (which would require constructing TextEdits with
    /// position arithmetic against the seed content).
    pub(crate) fn set_content_for_test(&mut self, content: Vec<u8>) {
        self.content = content;
    }

    /// Save the in-memory `content` to `path` atomically, refusing if
    /// `path/inode` no longer matches what was captured at open time.
    ///
    /// Pipeline (per `data-model.md §Validation rules R4 + R5`):
    ///
    /// 1. Stat `path`. Any error or non-regular-file → [`SaveOutcome::PathMissing`].
    ///    Inode delta from the open-time capture → [`SaveOutcome::InodeMismatch`].
    /// 2. Atomic POSIX write via [`atomic_write_with_hooks`] (tempfile
    ///    in same dir → write → fsync → rename → fsync(parent)).
    ///    Open / write / fsync-tempfile failures → [`SaveOutcome::TempfileIo`].
    ///    Rename / fsync-parent failures → [`SaveOutcome::RenameIo`].
    /// 3. Success → [`SaveOutcome::Saved`].
    ///
    /// Note: this method does NOT consult `self.last_dirty`. The
    /// clean-save no-op flow (R3) lives in `dispatch_buffer_save`
    /// BEFORE this method runs. `self.content` is read-only across
    /// any outcome; the on-disk file is byte-identical to its
    /// pre-call state on every pre-rename failure (atomic-rename
    /// invariant SC-504).
    pub fn save_to_disk(&self, path: &Path) -> SaveOutcome {
        self.save_to_disk_with_hooks(path, |_| Ok(()))
    }

    /// Test seam — same as [`Self::save_to_disk`] but accepts an
    /// injection hook passed through to [`atomic_write_with_hooks`].
    /// Used by `tests/e2e/buffer_save_atomic_invariant.rs` (SC-504)
    /// to verify the atomic-rename invariant under simulated I/O
    /// failure at each [`WriteStep`].
    pub(crate) fn save_to_disk_with_hooks<F>(&self, path: &Path, before: F) -> SaveOutcome
    where
        F: FnMut(WriteStep) -> Result<(), io::Error>,
    {
        // R4 — path/inode identity. Any stat error (not just
        // NotFound) collapses to PathMissing per data-model: an
        // unstatable target cannot be safely written through.
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return SaveOutcome::PathMissing,
        };
        if !metadata.file_type().is_file() {
            return SaveOutcome::PathMissing;
        }
        let actual_inode = metadata.ino();
        if actual_inode != self.inode {
            return SaveOutcome::InodeMismatch {
                expected: self.inode,
                actual: actual_inode,
            };
        }

        // R5 — atomic disk write. Map (WriteStep, io::Error) onto the
        // tempfile-vs-rename diagnostic split (research §9): the
        // open/write/fsync-tempfile bucket is operator-actionable
        // (disk full, permissions); the rename/fsync-parent bucket is
        // configuration-actionable (cross-filesystem, read-only mount).
        match atomic_write_with_hooks(path, &self.content, before) {
            Ok(()) => SaveOutcome::Saved {
                path: path.to_path_buf(),
            },
            Err((WriteStep::OpenTempfile, error))
            | Err((WriteStep::WriteContents, error))
            | Err((WriteStep::FsyncTempfile, error)) => SaveOutcome::TempfileIo { error },
            Err((WriteStep::RenameToTarget, error)) | Err((WriteStep::FsyncParentDir, error)) => {
                SaveOutcome::RenameIo { error }
            }
        }
    }
}

/// Outcome of [`BufferState::save_to_disk`].
///
/// A subset of [`crate::publisher::BufferSaveOutcome`] covering only
/// the disk-side outcomes (R4 + R5 of the validation pipeline). The
/// dispatcher converts `SaveOutcome` to the richer publisher-level
/// outcome by adding `entity` + `version` context.
#[derive(Debug)]
pub enum SaveOutcome {
    /// R5 success: target now byte-identical to `self.content`,
    /// parent directory `fsync`ed.
    Saved { path: PathBuf },
    /// R4 refusal: `path` exists and is a regular file but its inode
    /// no longer matches the buffer's captured open-time inode.
    /// `WEAVER-SAVE-005`.
    InodeMismatch { expected: u64, actual: u64 },
    /// R4 refusal: `path` does not exist on disk, is not a regular
    /// file, or stat failed for any other reason. `WEAVER-SAVE-006`.
    PathMissing,
    /// R5 failure in the tempfile open / write / fsync-tempfile
    /// steps. `WEAVER-SAVE-003`.
    TempfileIo { error: io::Error },
    /// R5 failure in the rename / fsync-parent steps.
    /// `WEAVER-SAVE-004`.
    RenameIo { error: io::Error },
}

/// Pure output of one observation tick — what the publisher needs to
/// decide which facts to re-assert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferObservation {
    pub byte_size: u64,
    pub dirty: bool,
    pub observable: bool,
}

/// The full bootstrap fact set for one opened buffer — the five
/// `(attribute, FactValue)` tuples the publisher emits once per file
/// before entering the poll loop.
///
/// This is the structural anchor for SC-306 (component discipline):
/// every value returned here is `FactValue::String`, `FactValue::U64`,
/// or `FactValue::Bool`; no `FactValue::Bytes` variant is reachable;
/// `buffer/path` renders the filesystem path, never file content.
/// The attribute→type map is:
///
/// - `buffer/path`       → `String` (canonical path, not file content)
/// - `buffer/byte-size`  → `U64`    (byte length of in-memory content)
/// - `buffer/dirty`      → `Bool`   (false at bootstrap)
/// - `buffer/observable` → `Bool`   (true at bootstrap)
/// - `buffer/version`    → `U64`    (applied-edit counter, 0 at
///   bootstrap; bumped by each accepted `EventPayload::BufferEdit`
///   in slice 004+; used to gate concurrent-edit conflict detection
///   when a stale edit references a pre-edit version. Present now
///   as forward-compat so slice 004 doesn't have to BREAKING-bump
///   the fact-family set.)
///
/// The publisher calls this from its bootstrap path so the invariant
/// is single-sourced; the `buffers/tests/component_discipline.rs`
/// proptest (T062) exercises the same seam under arbitrary content.
pub fn buffer_bootstrap_facts(state: &BufferState) -> [(&'static str, FactValue); 5] {
    [
        (
            "buffer/path",
            FactValue::String(state.path().display().to_string()),
        ),
        ("buffer/byte-size", FactValue::U64(state.byte_size())),
        ("buffer/dirty", FactValue::Bool(false)),
        ("buffer/observable", FactValue::Bool(true)),
        ("buffer/version", FactValue::U64(0)),
    ]
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

/// Endpoint of a [`Range`] — used by [`ApplyError::MidCodepointBoundary`]
/// to identify which side of the range tripped the validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundarySide {
    Start,
    End,
}

impl std::fmt::Display for BoundarySide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start => f.write_str("start"),
            Self::End => f.write_str("end"),
        }
    }
}

/// Categorised failure modes for [`BufferState::apply_edits`]. Each
/// variant maps to one of the validation rules R1..R6 in
/// `specs/004-buffer-edit/data-model.md §Validation rules`; the
/// publisher's reader-loop arm uses [`Self::reason`] to populate the
/// FR-018 `tracing::debug!` `reason` field.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApplyError {
    /// R2/R3 — endpoint references a line or character beyond the
    /// buffer's content (and is not the past-last virtual position).
    #[error("edit {edit_index} out of bounds: {detail}")]
    OutOfBounds { edit_index: usize, detail: String },

    /// R4 — endpoint character lands inside a multi-byte UTF-8 codepoint
    /// within the line's bytes.
    #[error(
        "edit {edit_index} {side} endpoint at line {line} character {character} \
         falls mid-codepoint within the line's bytes"
    )]
    MidCodepointBoundary {
        edit_index: usize,
        side: BoundarySide,
        line: u32,
        character: u32,
    },

    /// Two edits within the same batch overlap. Indices reference the
    /// original input order (not the post-sort order). FR-007 requires
    /// per-batch composition to have a single unambiguous interpretation;
    /// silent left-to-right composition is rejected in favour of an
    /// explicit error so the operator/agent can correct the batch.
    #[error("edits {first_index} and {second_index} overlap within the batch")]
    IntraBatchOverlap {
        first_index: usize,
        second_index: usize,
    },

    /// R5 — edit has an empty range AND an empty `new_text`; pure no-op
    /// at a point cursor. Distinguished from the empty-batch case (which
    /// is structurally an identity per FR-008).
    #[error("edit {edit_index} is a nothing-edit (empty range with empty new_text)")]
    NothingEdit { edit_index: usize },

    /// R1 — `range.start > range.end` (lexicographic compare on `(line,
    /// character)`).
    #[error("edit {edit_index} has start > end (swapped endpoints)")]
    SwappedEndpoints { edit_index: usize },

    /// R6 — `new_text` is not valid UTF-8. Structurally unreachable
    /// under safe Rust because [`TextEdit::new_text`] is `String` (UTF-8
    /// by construction); retained in the taxonomy so direct in-process
    /// callers building a `String` via `unsafe` have a defined error
    /// category rather than a panic.
    #[error("edit {edit_index} new_text is not valid UTF-8")]
    InvalidUtf8 { edit_index: usize },
}

impl ApplyError {
    /// Stable kebab-case reason category for FR-018 `tracing::debug!`.
    /// The publisher emits this verbatim in the `reason` field; tests
    /// in `tests/e2e/buffer_edit_*.rs` match against these strings.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::OutOfBounds { .. } => "validation-failure-out-of-bounds",
            Self::MidCodepointBoundary { .. } => "validation-failure-mid-codepoint-boundary",
            Self::IntraBatchOverlap { .. } => "validation-failure-intra-batch-overlap",
            Self::NothingEdit { .. } => "validation-failure-nothing-edit",
            Self::SwappedEndpoints { .. } => "validation-failure-swapped-endpoints",
            Self::InvalidUtf8 { .. } => "validation-failure-invalid-utf8",
        }
    }

    /// Per-edit input index for diagnostics when one applies; `None` for
    /// [`Self::IntraBatchOverlap`], which references a pair via its own
    /// `first_index` / `second_index` fields.
    pub fn edit_index(&self) -> Option<usize> {
        match self {
            Self::OutOfBounds { edit_index, .. }
            | Self::MidCodepointBoundary { edit_index, .. }
            | Self::NothingEdit { edit_index }
            | Self::SwappedEndpoints { edit_index }
            | Self::InvalidUtf8 { edit_index } => Some(*edit_index),
            Self::IntraBatchOverlap { .. } => None,
        }
    }
}

impl BufferState {
    /// Apply a batch of [`TextEdit`]s atomically against the in-memory
    /// `:content` component.
    ///
    /// Pipeline (per `specs/004-buffer-edit/research.md §3` and
    /// `data-model.md §Validation rules`):
    ///
    /// 1. **Per-edit validation** in input order, fail-fast: R1
    ///    swapped-endpoints → R5 nothing-edit → R2/R3 bounds → R4
    ///    codepoint boundaries.
    /// 2. **Sort by `range.start` (stable)** + adjacent overlap scan.
    ///    Tied starts where both edits are pure inserts (`start == end`)
    ///    are not overlap; both apply, in stable-sort order.
    /// 3. **Apply in descending byte-offset order** so earlier positions
    ///    are not shifted by later applications (LSP 3.17 convention).
    /// 4. **Recompute `memory_digest` once** at the end of the batch.
    ///
    /// Empty `edits` is a structural identity (FR-008): returns `Ok(())`
    /// and leaves `content` and `memory_digest` byte-identical.
    ///
    /// On `Err(ApplyError)` the buffer is unchanged — atomicity is
    /// guaranteed by validating the entire batch before any mutation.
    pub fn apply_edits(&mut self, edits: &[TextEdit]) -> Result<(), ApplyError> {
        if edits.is_empty() {
            return Ok(());
        }

        let line_index = LineIndex::build(&self.content);

        // Phase A — per-edit validation, fail-fast in input order.
        for (i, edit) in edits.iter().enumerate() {
            validate_edit(i, edit, &line_index)?;
        }

        // Phase B — sort by start (stable) + adjacent overlap scan.
        let mut sorted: Vec<(usize, &TextEdit)> = edits.iter().enumerate().collect();
        sorted.sort_by_key(|(_, e)| e.range.start);
        for window in sorted.windows(2) {
            let (i_first, e_first) = window[0];
            let (i_second, e_second) = window[1];
            let first_is_insert = e_first.range.start == e_first.range.end;
            let second_is_insert = e_second.range.start == e_second.range.end;
            let strictly_overlaps = e_first.range.end > e_second.range.start;
            let tied_starts = e_first.range.start == e_second.range.start;
            let tied_with_non_insert = tied_starts && !(first_is_insert && second_is_insert);
            if strictly_overlaps || tied_with_non_insert {
                let (a, b) = if i_first <= i_second {
                    (i_first, i_second)
                } else {
                    (i_second, i_first)
                };
                return Err(ApplyError::IntraBatchOverlap {
                    first_index: a,
                    second_index: b,
                });
            }
        }

        // Phase C — apply in descending byte-offset order. Compute byte
        // offsets while line_index is still valid, then mutate.
        let plan: Vec<(usize, usize, &str)> = sorted
            .iter()
            .rev()
            .map(|(_, e)| {
                let start = line_index
                    .byte_offset(e.range.start)
                    .expect("range.start validated in Phase A");
                let end = line_index
                    .byte_offset(e.range.end)
                    .expect("range.end validated in Phase A");
                (start, end, e.new_text.as_str())
            })
            .collect();
        drop(line_index);

        for (start, end, new_text) in plan {
            self.content.splice(start..end, new_text.bytes());
        }

        // Phase D — single digest recompute. Buffer-size cost dominates;
        // batch size is irrelevant.
        self.memory_digest = Sha256::digest(&self.content).into();
        Ok(())
    }
}

/// Cached `(line → start-byte)` index built once per `apply_edits`
/// invocation. `line_count` follows the data-model rule: a trailing
/// `\n` does not contribute a phantom line beyond the lines it
/// terminates.
struct LineIndex<'a> {
    content: &'a [u8],
    line_starts: Vec<usize>,
    line_count: u32,
}

impl<'a> LineIndex<'a> {
    fn build(content: &'a [u8]) -> Self {
        let mut line_starts = Vec::with_capacity(16);
        line_starts.push(0);
        for (i, &b) in content.iter().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        // A trailing `\n` would push `content.len()` as the start of a
        // virtual line that doesn't exist (data-model `+0` clause). Drop it.
        if matches!(content.last(), Some(&b'\n')) {
            line_starts.pop();
        }
        let line_count = u32::try_from(line_starts.len()).unwrap_or(u32::MAX);
        Self {
            content,
            line_starts,
            line_count,
        }
    }

    /// Byte-length of `line`'s content, excluding any terminating `\n`.
    /// Caller MUST ensure `line < self.line_count`.
    fn line_byte_length(&self, line: u32) -> usize {
        let line_idx = line as usize;
        let start = self.line_starts[line_idx];
        if line_idx + 1 < self.line_starts.len() {
            self.line_starts[line_idx + 1] - 1 - start
        } else if matches!(self.content.last(), Some(&b'\n')) {
            self.content.len() - 1 - start
        } else {
            self.content.len() - start
        }
    }

    fn line_bytes(&self, line: u32) -> &[u8] {
        let start = self.line_starts[line as usize];
        let len = self.line_byte_length(line);
        &self.content[start..start + len]
    }

    /// Convert a `Position` to a byte offset within `content`. Returns
    /// `None` when the position lies outside the buffer (the past-last
    /// virtual position `(line_count, 0)` is in-bounds and maps to
    /// `content.len()`).
    fn byte_offset(&self, pos: Position) -> Option<usize> {
        if pos.line < self.line_count {
            let line_start = self.line_starts[pos.line as usize];
            let len = self.line_byte_length(pos.line);
            let ch = pos.character as usize;
            (ch <= len).then_some(line_start + ch)
        } else if pos.line == self.line_count && pos.character == 0 {
            Some(self.content.len())
        } else {
            None
        }
    }
}

fn validate_edit(i: usize, edit: &TextEdit, idx: &LineIndex<'_>) -> Result<(), ApplyError> {
    let r = edit.range;

    if r.start > r.end {
        return Err(ApplyError::SwappedEndpoints { edit_index: i });
    }
    if r.start == r.end && edit.new_text.is_empty() {
        return Err(ApplyError::NothingEdit { edit_index: i });
    }
    check_position_bounds(i, BoundarySide::Start, r.start, idx)?;
    check_position_bounds(i, BoundarySide::End, r.end, idx)?;
    check_codepoint_boundary(i, BoundarySide::Start, r.start, idx)?;
    check_codepoint_boundary(i, BoundarySide::End, r.end, idx)?;
    // R6 (InvalidUtf8) is structurally unreachable under safe Rust:
    // `TextEdit::new_text: String` is UTF-8 by construction. The variant
    // exists in the taxonomy for direct in-process callers (e.g.,
    // `unsafe`-built `String`s) so the error has a category rather than
    // a panic.
    Ok(())
}

fn check_position_bounds(
    edit_index: usize,
    side: BoundarySide,
    pos: Position,
    idx: &LineIndex<'_>,
) -> Result<(), ApplyError> {
    if pos.line < idx.line_count {
        let len = idx.line_byte_length(pos.line);
        if pos.character as usize > len {
            return Err(ApplyError::OutOfBounds {
                edit_index,
                detail: format!(
                    "{side} character {} exceeds line {} content length {len}",
                    pos.character, pos.line
                ),
            });
        }
        Ok(())
    } else if pos.line == idx.line_count && pos.character == 0 {
        Ok(())
    } else {
        Err(ApplyError::OutOfBounds {
            edit_index,
            detail: format!(
                "{side} line {} exceeds buffer line count {}",
                pos.line, idx.line_count
            ),
        })
    }
}

fn check_codepoint_boundary(
    edit_index: usize,
    side: BoundarySide,
    pos: Position,
    idx: &LineIndex<'_>,
) -> Result<(), ApplyError> {
    // Past-last virtual position is at content.len() — always a boundary.
    if pos.line == idx.line_count {
        return Ok(());
    }
    let line_bytes = idx.line_bytes(pos.line);
    if !is_codepoint_boundary(line_bytes, pos.character as usize) {
        return Err(ApplyError::MidCodepointBoundary {
            edit_index,
            side,
            line: pos.line,
            character: pos.character,
        });
    }
    Ok(())
}

/// Mirrors `str::is_char_boundary` semantics over raw bytes so the
/// validator remains defined on buffers whose contents may not be valid
/// UTF-8. A continuation byte matches `0b10xxxxxx`.
fn is_codepoint_boundary(bytes: &[u8], index: usize) -> bool {
    if index == 0 || index >= bytes.len() {
        return true;
    }
    bytes[index] & 0b1100_0000 != 0b1000_0000
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
    fn buffer_state_open_captures_inode_matching_stat() {
        // Slice 005: BufferState::open captures the inode at open time
        // via std::os::unix::fs::MetadataExt::ino(). The captured value
        // must equal an independent stat(2) of the same path.
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(b"x").expect("write");
        let canonical = std::fs::canonicalize(f.path()).expect("canonicalize");

        let stat_inode = std::fs::metadata(&canonical).expect("stat").ino();
        let state = BufferState::open(canonical).expect("open");

        assert_eq!(state.inode(), stat_inode);
        assert_ne!(state.inode(), 0, "inode must be non-zero on a real file");
    }

    #[test]
    fn buffer_state_open_through_symlink_captures_target_inode() {
        // POSIX `metadata` follows symlinks (vs `symlink_metadata`
        // which does not). Opening a symlink path must capture the
        // *target's* inode — atomic-replace detection relies on this.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target.txt");
        std::fs::write(&target, b"target content").expect("write target");
        let target_canonical = std::fs::canonicalize(&target).expect("canonicalize target");

        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target_canonical, &link).expect("symlink");

        let target_inode = std::fs::metadata(&target_canonical)
            .expect("stat target")
            .ino();
        // Open via the symlink's path. `BufferState::open` requires a
        // canonical path; canonicalize resolves the symlink, so the
        // captured inode is the target's regardless. Pin the property
        // explicitly.
        let opened_canonical = std::fs::canonicalize(&link).expect("canonicalize link");
        let state = BufferState::open(opened_canonical).expect("open through link");
        assert_eq!(state.inode(), target_inode);
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

    // ───────────────────────────────────────────────────────────────────
    // Slice-004: BufferState::apply_edits + ApplyError taxonomy (T007).
    // ───────────────────────────────────────────────────────────────────

    use weaver_core::types::edit::{Position as EditPos, Range as EditRange, TextEdit};

    fn pos(line: u32, character: u32) -> EditPos {
        EditPos { line, character }
    }

    fn rng(start: EditPos, end: EditPos) -> EditRange {
        EditRange { start, end }
    }

    fn edit(start: EditPos, end: EditPos, new_text: &str) -> TextEdit {
        TextEdit {
            range: rng(start, end),
            new_text: new_text.into(),
        }
    }

    fn state_from_bytes(bytes: &[u8]) -> BufferState {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(bytes).expect("write");
        let canonical = std::fs::canonicalize(f.path()).expect("canonicalize");
        BufferState::open(canonical).expect("open")
    }

    #[test]
    fn apply_edits_empty_batch_is_structural_identity() {
        // FR-008: empty `edits: []` returns Ok(()) without mutating
        // content or memory_digest.
        let mut s = state_from_bytes(b"hello world\n");
        let content_before = s.content().to_vec();
        let digest_before = *s.memory_digest();
        s.apply_edits(&[]).expect("empty batch must be Ok");
        assert_eq!(s.content(), content_before.as_slice());
        assert_eq!(s.memory_digest(), &digest_before);
    }

    #[test]
    fn apply_edits_pure_insert_at_start() {
        let mut s = state_from_bytes(b"world");
        s.apply_edits(&[edit(pos(0, 0), pos(0, 0), "hello ")])
            .expect("pure insert at start");
        assert_eq!(s.content(), b"hello world");
    }

    #[test]
    fn apply_edits_pure_delete() {
        // Delete bytes 5..11 of "hello world" (the " world" suffix).
        let mut s = state_from_bytes(b"hello world");
        s.apply_edits(&[edit(pos(0, 5), pos(0, 11), "")])
            .expect("pure delete");
        assert_eq!(s.content(), b"hello");
    }

    #[test]
    fn apply_edits_replace() {
        let mut s = state_from_bytes(b"hello world");
        s.apply_edits(&[edit(pos(0, 0), pos(0, 5), "HOWDY")])
            .expect("replace");
        assert_eq!(s.content(), b"HOWDY world");
    }

    #[test]
    fn apply_edits_batched_descending_apply_preserves_input_positions() {
        // Three insertions at original positions 0, 3, 6 of "AAABBBCCC".
        // If apply order weren't descending, later edits would shift
        // earlier ones; the test asserts each insertion lands at its
        // ORIGINAL byte offset.
        let mut s = state_from_bytes(b"AAABBBCCC");
        s.apply_edits(&[
            edit(pos(0, 0), pos(0, 0), ">"),
            edit(pos(0, 3), pos(0, 3), "|"),
            edit(pos(0, 6), pos(0, 6), "|"),
        ])
        .expect("batched insert");
        assert_eq!(s.content(), b">AAA|BBB|CCC");
    }

    #[test]
    fn apply_edits_supports_insert_at_past_last_virtual_position() {
        // line_count = 1 for "abc\n" (trailing \n does NOT add a phantom line).
        // (1, 0) is the past-last virtual position; legal for insertion.
        let mut s = state_from_bytes(b"abc\n");
        s.apply_edits(&[edit(pos(1, 0), pos(1, 0), "def")])
            .expect("insert at past-last position");
        assert_eq!(s.content(), b"abc\ndef");
    }

    #[test]
    fn apply_edits_two_pure_inserts_at_same_position_both_apply_in_input_order() {
        // Tied starts where both edits are pure inserts are NOT overlap.
        // Stable sort preserves input order; descending-offset apply
        // produces input-order left-to-right in the final content.
        let mut s = state_from_bytes(b"world");
        s.apply_edits(&[
            edit(pos(0, 0), pos(0, 0), "hello "),
            edit(pos(0, 0), pos(0, 0), "WAVE "),
        ])
        .expect("two tied pure inserts");
        assert_eq!(s.content(), b"hello WAVE world");
    }

    #[test]
    fn apply_edits_rejects_swapped_endpoints() {
        let mut s = state_from_bytes(b"hello world");
        let err = s
            .apply_edits(&[edit(pos(0, 5), pos(0, 0), "x")])
            .expect_err("swapped endpoints must be rejected");
        assert!(matches!(
            err,
            ApplyError::SwappedEndpoints { edit_index: 0 }
        ));
        assert_eq!(s.content(), b"hello world");
    }

    #[test]
    fn apply_edits_rejects_out_of_bounds_line() {
        // Buffer "hello" has line_count = 1; line 5 is far past it and
        // not the past-last virtual position.
        let mut s = state_from_bytes(b"hello");
        let err = s
            .apply_edits(&[edit(pos(0, 0), pos(5, 0), "x")])
            .expect_err("out-of-bounds line must be rejected");
        match err {
            ApplyError::OutOfBounds { edit_index, detail } => {
                assert_eq!(edit_index, 0);
                assert!(detail.contains("line"), "detail mentions line: {detail}");
            }
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    #[test]
    fn apply_edits_rejects_out_of_bounds_character() {
        // Buffer "hi" — line 0 length is 2 bytes. Character 5 is OOB.
        let mut s = state_from_bytes(b"hi");
        let err = s
            .apply_edits(&[edit(pos(0, 5), pos(0, 5), "x")])
            .expect_err("out-of-bounds character must be rejected");
        match err {
            ApplyError::OutOfBounds { edit_index, detail } => {
                assert_eq!(edit_index, 0);
                assert!(
                    detail.contains("character"),
                    "detail mentions character: {detail}"
                );
            }
            other => panic!("expected OutOfBounds, got {other:?}"),
        }
    }

    #[test]
    fn apply_edits_rejects_mid_codepoint_boundary() {
        // "héllo" UTF-8: h(0x68) é(0xC3 0xA9) l(0x6C) l(0x6C) o(0x6F).
        // Position character=2 lands inside é (0xA9 is a continuation byte).
        let mut s = state_from_bytes("héllo".as_bytes());
        let err = s
            .apply_edits(&[edit(pos(0, 2), pos(0, 2), "x")])
            .expect_err("mid-codepoint must be rejected");
        match err {
            ApplyError::MidCodepointBoundary {
                edit_index,
                side,
                line,
                character,
            } => {
                assert_eq!(edit_index, 0);
                assert_eq!(side, BoundarySide::Start);
                assert_eq!(line, 0);
                assert_eq!(character, 2);
            }
            other => panic!("expected MidCodepointBoundary, got {other:?}"),
        }
    }

    #[test]
    fn apply_edits_rejects_intra_batch_overlap() {
        // Edits 0:0..0:5 and 0:3..0:7 overlap in [3, 5).
        let mut s = state_from_bytes(b"hello world");
        let err = s
            .apply_edits(&[
                edit(pos(0, 0), pos(0, 5), "X"),
                edit(pos(0, 3), pos(0, 7), "Y"),
            ])
            .expect_err("overlap must be rejected");
        assert!(matches!(
            err,
            ApplyError::IntraBatchOverlap {
                first_index: 0,
                second_index: 1,
            }
        ));
    }

    #[test]
    fn apply_edits_rejects_intra_batch_overlap_tied_start_with_non_insert() {
        // Tied starts where one edit is NOT a pure insert → overlap per
        // data-model §Validation rules: "Tied starts ... are NOT overlap
        // unless one of them has start < end."
        let mut s = state_from_bytes(b"hello world");
        let err = s
            .apply_edits(&[
                edit(pos(0, 0), pos(0, 5), "REPLACE"),
                edit(pos(0, 0), pos(0, 0), "INSERT "),
            ])
            .expect_err("tied start with non-insert must be rejected");
        assert!(matches!(
            err,
            ApplyError::IntraBatchOverlap {
                first_index: 0,
                second_index: 1,
            }
        ));
    }

    #[test]
    fn apply_edits_rejects_nothing_edit() {
        let mut s = state_from_bytes(b"hello");
        let err = s
            .apply_edits(&[edit(pos(0, 2), pos(0, 2), "")])
            .expect_err("nothing-edit must be rejected");
        assert!(matches!(err, ApplyError::NothingEdit { edit_index: 0 }));
    }

    #[test]
    fn apply_edits_invalid_utf8_variant_is_in_taxonomy() {
        // R6 is structurally unreachable under safe Rust (`String` is
        // UTF-8 by construction). Pin Display + reason + edit_index so
        // direct in-process callers get a well-defined error category.
        let err = ApplyError::InvalidUtf8 { edit_index: 7 };
        assert_eq!(err.reason(), "validation-failure-invalid-utf8");
        assert_eq!(err.edit_index(), Some(7));
        assert!(format!("{err}").contains("not valid UTF-8"));
    }

    #[test]
    fn apply_edits_preserves_digest_invariant_on_accept() {
        // The slice-003 invariant `memory_digest == sha256(content)`
        // MUST hold after any accepted batch.
        let mut s = state_from_bytes(b"hello world");
        s.apply_edits(&[edit(pos(0, 0), pos(0, 0), "PRE-")])
            .expect("accept");
        let expected: [u8; 32] = Sha256::digest(s.content()).into();
        assert_eq!(s.memory_digest(), &expected);
    }

    #[test]
    fn apply_edits_state_unchanged_on_rejection() {
        // Atomicity: if any edit in the batch fails validation, neither
        // content nor memory_digest changes.
        let mut s = state_from_bytes(b"hello world");
        let content_before = s.content().to_vec();
        let digest_before = *s.memory_digest();
        let _ = s
            .apply_edits(&[
                edit(pos(0, 0), pos(0, 5), "ok"),
                edit(pos(0, 99), pos(0, 99), "boom"),
            ])
            .expect_err("invalid second edit must reject whole batch");
        assert_eq!(s.content(), content_before.as_slice());
        assert_eq!(s.memory_digest(), &digest_before);
    }

    #[test]
    fn apply_edits_handles_buffer_with_trailing_newline() {
        // "abc\n" → line_count = 1, line 0 length = 3 (excludes the \n).
        // Inserting at character=3 (end-of-line position) keeps the \n.
        let mut s = state_from_bytes(b"abc\n");
        s.apply_edits(&[edit(pos(0, 3), pos(0, 3), "X")])
            .expect("end-of-line insert");
        assert_eq!(s.content(), b"abcX\n");
    }

    #[test]
    fn apply_edits_handles_empty_buffer() {
        // Empty buffer: line_count = 1, line 0 is empty, (0,0)..(0,0)
        // is the only valid insert position (and (1,0)..(1,0) past-last).
        let mut s = state_from_bytes(b"");
        s.apply_edits(&[edit(pos(0, 0), pos(0, 0), "first")])
            .expect("empty-buffer insert");
        assert_eq!(s.content(), b"first");
    }

    #[test]
    fn apply_error_reason_categories_match_wire_vocabulary() {
        // Pin the FR-018 reason strings — they're the public wire
        // vocabulary the publisher emits and e2e tests grep for.
        assert_eq!(
            ApplyError::OutOfBounds {
                edit_index: 0,
                detail: String::new(),
            }
            .reason(),
            "validation-failure-out-of-bounds",
        );
        assert_eq!(
            ApplyError::MidCodepointBoundary {
                edit_index: 0,
                side: BoundarySide::Start,
                line: 0,
                character: 0,
            }
            .reason(),
            "validation-failure-mid-codepoint-boundary",
        );
        assert_eq!(
            ApplyError::IntraBatchOverlap {
                first_index: 0,
                second_index: 1,
            }
            .reason(),
            "validation-failure-intra-batch-overlap",
        );
        assert_eq!(
            ApplyError::NothingEdit { edit_index: 0 }.reason(),
            "validation-failure-nothing-edit",
        );
        assert_eq!(
            ApplyError::SwappedEndpoints { edit_index: 0 }.reason(),
            "validation-failure-swapped-endpoints",
        );
        assert_eq!(
            ApplyError::InvalidUtf8 { edit_index: 0 }.reason(),
            "validation-failure-invalid-utf8",
        );
    }

    #[test]
    fn apply_error_edit_index_returns_none_for_intra_batch_overlap() {
        // Pair-shaped error has its own first_index/second_index fields;
        // edit_index() returns None to signal the structural difference.
        let err = ApplyError::IntraBatchOverlap {
            first_index: 2,
            second_index: 5,
        };
        assert_eq!(err.edit_index(), None);
    }

    #[test]
    fn boundary_side_displays_kebab_case() {
        // Used in OutOfBounds.detail and human-readable diagnostics.
        assert_eq!(format!("{}", BoundarySide::Start), "start");
        assert_eq!(format!("{}", BoundarySide::End), "end");
    }

    // ───────────────────────────────────────────────────────────────────
    // Slice-005: BufferState::save_to_disk + SaveOutcome taxonomy (T016).
    // ───────────────────────────────────────────────────────────────────

    #[test]
    fn save_to_disk_saved_happy_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original").expect("seed");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");

        let mut state = BufferState::open(canonical.clone()).expect("open");
        state
            .apply_edits(&[edit(pos(0, 0), pos(0, 8), "REWRITTEN")])
            .expect("rewrite");

        match state.save_to_disk(&canonical) {
            SaveOutcome::Saved { path } => assert_eq!(path, canonical),
            other => panic!("expected Saved, got {other:?}"),
        }
        assert_eq!(std::fs::read(&canonical).expect("read"), b"REWRITTEN");
    }

    #[test]
    fn save_to_disk_inode_mismatch_after_external_replace() {
        // External atomic-replace (the SC-502 acceptance scenario):
        // a different file is renamed over the path, so the path
        // exists but the inode has changed.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original").expect("seed");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");

        let state = BufferState::open(canonical.clone()).expect("open");
        let captured_inode = state.inode();

        let replacement = dir.path().join("replacement.txt");
        std::fs::write(&replacement, b"different").expect("write replacement");
        std::fs::rename(&replacement, &canonical).expect("atomic-replace");
        let new_inode = std::fs::metadata(&canonical).expect("stat").ino();
        assert_ne!(
            captured_inode, new_inode,
            "test precondition: inode changed"
        );

        match state.save_to_disk(&canonical) {
            SaveOutcome::InodeMismatch { expected, actual } => {
                assert_eq!(expected, captured_inode);
                assert_eq!(actual, new_inode);
            }
            other => panic!("expected InodeMismatch, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&canonical).expect("read"),
            b"different",
            "external replacement is preserved; save was refused"
        );
    }

    #[test]
    fn save_to_disk_path_missing_after_external_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original").expect("seed");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");

        let state = BufferState::open(canonical.clone()).expect("open");
        std::fs::remove_file(&canonical).expect("delete");

        assert!(matches!(
            state.save_to_disk(&canonical),
            SaveOutcome::PathMissing
        ));
    }

    #[test]
    fn save_to_disk_path_missing_when_target_is_directory() {
        // Non-regular file at the path is treated as missing for
        // save purposes (data-model R4 second bullet).
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original").expect("seed");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");
        let state = BufferState::open(canonical.clone()).expect("open");

        // Replace the regular file with a directory at the same path.
        std::fs::remove_file(&canonical).expect("rm file");
        std::fs::create_dir(&canonical).expect("mkdir");

        assert!(matches!(
            state.save_to_disk(&canonical),
            SaveOutcome::PathMissing
        ));
    }

    #[test]
    fn save_to_disk_tempfile_io_via_hook_injection_preserves_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original").expect("seed");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");

        let mut state = BufferState::open(canonical.clone()).expect("open");
        state
            .apply_edits(&[edit(pos(0, 0), pos(0, 8), "MUTATED")])
            .expect("mutate in-memory");

        let outcome = state.save_to_disk_with_hooks(&canonical, |s| {
            if s == WriteStep::WriteContents {
                Err(io::Error::other("simulated ENOSPC"))
            } else {
                Ok(())
            }
        });
        match outcome {
            SaveOutcome::TempfileIo { .. } => {}
            other => panic!("expected TempfileIo, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&canonical).expect("read"),
            b"original",
            "atomic-rename invariant: pre-rename failure leaves disk byte-identical"
        );
    }

    #[test]
    fn save_to_disk_rename_io_via_hook_injection_preserves_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original").expect("seed");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");

        let mut state = BufferState::open(canonical.clone()).expect("open");
        state
            .apply_edits(&[edit(pos(0, 0), pos(0, 8), "MUTATED")])
            .expect("mutate in-memory");

        let outcome = state.save_to_disk_with_hooks(&canonical, |s| {
            if s == WriteStep::RenameToTarget {
                Err(io::Error::other("simulated EXDEV"))
            } else {
                Ok(())
            }
        });
        match outcome {
            SaveOutcome::RenameIo { .. } => {}
            other => panic!("expected RenameIo, got {other:?}"),
        }
        assert_eq!(
            std::fs::read(&canonical).expect("read"),
            b"original",
            "atomic-rename invariant: pre-rename failure leaves disk byte-identical"
        );
    }

    #[test]
    fn save_to_disk_buffer_state_content_unchanged_after_failure() {
        // Purity invariant: BufferState::content is read-only across
        // every SaveOutcome failure branch.
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original").expect("seed");
        let canonical = std::fs::canonicalize(&target).expect("canonicalize");

        let mut state = BufferState::open(canonical.clone()).expect("open");
        state
            .apply_edits(&[edit(pos(0, 0), pos(0, 8), "MUTATED")])
            .expect("mutate");
        let content_before = state.content().to_vec();

        std::fs::remove_file(&canonical).expect("delete");
        let _ = state.save_to_disk(&canonical);

        assert_eq!(state.content(), content_before.as_slice());
    }
}
