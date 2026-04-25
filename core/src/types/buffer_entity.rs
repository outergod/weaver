//! Buffer-namespace entity-ID derivation.
//!
//! Slice-002 introduced two reserved namespace bits: bit 63 for the
//! repo namespace (owned by `weaver-git-watcher`) and bit 62 for the
//! per-process service-instance namespace (used by both
//! `weaver-git-watcher` and `weaver-buffers` instance entities).
//! Slice-003 added bit 61 for buffer entities derived from a file's
//! canonical path.
//!
//! [`buffer_entity_ref`] originally lived in
//! `weaver-buffers::model`; slice-004 lifted it to `weaver_core` so
//! the `weaver edit` CLI handler can derive the same entity-id
//! without a cross-crate runtime dependency on `weaver-buffers`
//! (which would be a layering inversion AND a circular Cargo
//! dependency, since `weaver-buffers` depends on `weaver_core`).
//! `weaver-buffers::model` re-exports the function for slice-003
//! callers (publisher + tests); the canonical implementation lives
//! here.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use crate::types::entity_ref::EntityRef;

/// Buffer-namespace bit (slice 003). Set on every entity derived from
/// a buffer's canonical filesystem path; distinct from
/// [`INSTANCE_NAMESPACE_BIT`] and [`REPO_NAMESPACE_BIT`] so a low-
/// order hash collision cannot accidentally claim either of them.
pub const BUFFER_NAMESPACE_BIT: u64 = 1 << 61;

/// Watcher-instance-namespace bit (slice 002). Set on every entity
/// representing a per-process service instance. The TUI/inspect
/// machinery distinguishes instance kinds (git-watcher vs
/// weaver-buffers vs ...) by asserted facts, not by entity-id bits.
pub const INSTANCE_NAMESPACE_BIT: u64 = 1 << 62;

/// Repo-namespace bit (slice 002, owned by `weaver-git-watcher`).
/// Buffer + instance derivations clear this bit so a low-order hash
/// never accidentally claims the repo namespace.
pub const REPO_NAMESPACE_BIT: u64 = 1 << 63;

/// Derive a stable [`EntityRef`] for a buffer entity from its
/// canonicalised absolute path.
///
/// The caller MUST pass an already-canonicalised path (e.g., the
/// output of [`std::fs::canonicalize`]). This function is pure (no
/// I/O, no implicit canonicalisation), so the entity id is
/// deterministic for any canonical path.
pub fn buffer_entity_ref(path: &Path) -> EntityRef {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let h =
        (hasher.finish() | BUFFER_NAMESPACE_BIT) & !(INSTANCE_NAMESPACE_BIT | REPO_NAMESPACE_BIT);
    EntityRef::new(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn buffer_entity_sets_bit_61_clears_bits_62_and_63() {
        for path in [
            "/tmp/a",
            "/home/alex/code/weaver/core/src/lib.rs",
            "/",
            "/this/is/a/very/long/canonical/path/that/exercises/the/hasher",
        ] {
            let e = buffer_entity_ref(&p(path)).as_u64();
            assert!(
                e & BUFFER_NAMESPACE_BIT != 0,
                "bit 61 must be set for {path}"
            );
            assert!(
                e & INSTANCE_NAMESPACE_BIT == 0,
                "bit 62 must be clear for {path}"
            );
            assert!(
                e & REPO_NAMESPACE_BIT == 0,
                "bit 63 must be clear for {path}"
            );
        }
    }

    #[test]
    fn buffer_entity_is_deterministic() {
        let path = p("/home/alex/file.txt");
        assert_eq!(buffer_entity_ref(&path), buffer_entity_ref(&path));
    }

    #[test]
    fn buffer_entity_distinguishes_paths() {
        let a = buffer_entity_ref(&p("/tmp/a"));
        let b = buffer_entity_ref(&p("/tmp/b"));
        assert_ne!(a, b);
    }
}
