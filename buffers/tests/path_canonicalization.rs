//! T064 — property tests for `buffer_entity_ref` determinism and
//! reserved-bit invariants, plus canonicalization idempotence at the
//! entity-id layer.
//!
//! Covers `specs/003-buffer-service/data-model.md` validation rules:
//!
//! 1. Path canonicalization idempotence:
//!    `buffer_entity_ref(canonicalize(p)) == buffer_entity_ref(canonicalize(p))`
//!    for any valid path `p`.
//! 2. Path-based entity equality falls out of rule 1 by the pure-function
//!    nature of `buffer_entity_ref` — distinct encodings of the same
//!    canonical path produce the same entity.
//! 3. Reserved-bit invariants: every derived entity has bit 61 set
//!    (buffer namespace) and bits 62 / 63 cleared (watcher-instance and
//!    repo namespaces are owned by slice-002 derivations and slice-001
//!    repo entities respectively).
//!
//! Existing hand-picked unit coverage (four paths) lives in
//! `buffers/src/model.rs::tests`; the proptest here exercises the same
//! invariants across a wider path-string space, guarding against a
//! future hash-function swap regressing only on rare path shapes.

use std::io::Write;
use std::path::PathBuf;

use proptest::prelude::*;
use tempfile::NamedTempFile;
use weaver_buffers::model::buffer_entity_ref;

/// Path-shaped string strategy: single-segment, relative multi-segment,
/// and absolute multi-segment. Segment alphabet is restricted to
/// `[A-Za-z0-9._-]` so the strings round-trip cleanly through
/// `PathBuf::from` and through OS APIs that the unit-level tests might
/// later reach for. The invariants we assert here are pure-function
/// properties of `buffer_entity_ref` — they do not require the path to
/// exist on disk.
fn arb_path_string() -> impl Strategy<Value = String> {
    prop_oneof![
        "[A-Za-z0-9._-]{1,16}".prop_map(|s| s),
        proptest::collection::vec("[A-Za-z0-9._-]{1,12}", 1..=6).prop_map(|segs| segs.join("/")),
        proptest::collection::vec("[A-Za-z0-9._-]{1,12}", 1..=6)
            .prop_map(|segs| format!("/{}", segs.join("/"))),
    ]
}

const BUFFER_NAMESPACE_BIT: u64 = 1 << 61;
const INSTANCE_NAMESPACE_BIT: u64 = 1 << 62;
const REPO_NAMESPACE_BIT: u64 = 1 << 63;

proptest! {
    /// Rule 3 — reserved-bit invariants hold across the path space.
    #[test]
    fn buffer_entity_ref_reserved_bits_hold_for_arbitrary_paths(
        p in arb_path_string(),
    ) {
        let path = PathBuf::from(&p);
        let e = buffer_entity_ref(&path).as_u64();
        prop_assert_ne!(
            e & BUFFER_NAMESPACE_BIT,
            0,
            "bit 61 (buffer namespace) must be set for path {:?}",
            path,
        );
        prop_assert_eq!(
            e & INSTANCE_NAMESPACE_BIT,
            0,
            "bit 62 (watcher-instance namespace) must be clear for path {:?}",
            path,
        );
        prop_assert_eq!(
            e & REPO_NAMESPACE_BIT,
            0,
            "bit 63 (repo namespace) must be clear for path {:?}",
            path,
        );
    }

    /// Rule 2 fallout — determinism of the pure function.
    ///
    /// Two invocations with the same input must produce the same entity.
    /// A regression here would mean `buffer_entity_ref` picked up hidden
    /// state (e.g., a `DefaultHasher` reseed, environment drift).
    #[test]
    fn buffer_entity_ref_is_deterministic_for_arbitrary_paths(
        p in arb_path_string(),
    ) {
        let path = PathBuf::from(&p);
        prop_assert_eq!(buffer_entity_ref(&path), buffer_entity_ref(&path));
    }

    /// Rule 1 — canonicalize-then-entity is stable under double
    /// canonicalization. Requires a real file, so the proptest creates
    /// a tempfile with arbitrary content, canonicalizes twice, and
    /// verifies the entity matches across the second canonicalization.
    #[test]
    fn canonicalize_idempotence_preserves_entity(
        content in proptest::collection::vec(any::<u8>(), 0..=1024),
    ) {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(&content).expect("write content");
        f.flush().expect("flush");
        let once = std::fs::canonicalize(f.path()).expect("canonicalize(f)");
        let twice = std::fs::canonicalize(&once).expect("canonicalize(canonical)");
        prop_assert_eq!(&once, &twice);
        prop_assert_eq!(buffer_entity_ref(&once), buffer_entity_ref(&twice));
    }
}
