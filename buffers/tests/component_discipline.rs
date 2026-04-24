//! T062 — SC-306 component discipline proptest.
//!
//! Slice-003's defining value-space invariant: every `FactValue` the
//! buffer service emits is `String`, `U64`, or `Bool`. No
//! `FactValue::Bytes`; no `FactValue::String` carrying the opened
//! file's content. The only String emission at the buffer layer is
//! `buffer/path`, which renders the filesystem path.
//!
//! The structural seam is [`weaver_buffers::model::buffer_bootstrap_facts`],
//! which the publisher consumes on its bootstrap path. Exercising the
//! seam directly lets us assert the attribute→type map across
//! randomised content without standing up a bus. Poll-tick transitions
//! are pure `FactValue::Bool` constructions — Rust's type system pins
//! those; the content-adjacent emissions are what need a property test.
//!
//! Invariants pinned here:
//!
//! 1. Exactly five facts: `buffer/path`, `buffer/byte-size`,
//!    `buffer/dirty`, `buffer/observable`, `buffer/version`. No
//!    duplicates, no extras.
//! 2. Variant discipline: every value is `String | U64 | Bool`.
//! 3. Attribute→type map:
//!    - `buffer/path` → `String` equal to `path.display().to_string()`.
//!    - `buffer/byte-size` → `U64` equal to `content.len() as u64`.
//!    - `buffer/dirty` → `Bool(false)` (bootstrap is clean).
//!    - `buffer/observable` → `Bool(true)` (bootstrap is observable).
//!    - `buffer/version` → `U64(0)` (bootstrap applied-edit counter
//!      starts at 0; slice 004+ bumps on each accepted edit).
//! 4. No content leakage: the `buffer/path` String value is not the
//!    file's content. FR-002a is the defining invariant this guards.

use std::io::Write;

use proptest::prelude::*;
use tempfile::NamedTempFile;
use weaver_buffers::model::{BufferState, buffer_bootstrap_facts};
use weaver_core::types::fact::FactValue;

proptest! {
    #[test]
    fn bootstrap_facts_obey_sc_306_discipline(
        content in proptest::collection::vec(any::<u8>(), 0..=4096),
    ) {
        let mut f = NamedTempFile::new().expect("tempfile");
        f.write_all(&content).expect("write content");
        f.flush().expect("flush");
        let canonical = std::fs::canonicalize(f.path()).expect("canonicalize");

        let state = BufferState::open(canonical.clone()).expect("open canonical tempfile");
        let facts = buffer_bootstrap_facts(&state);

        // (1) Five distinct attributes, alphabetically.
        prop_assert_eq!(facts.len(), 5);
        let mut attrs: Vec<&str> = facts.iter().map(|(a, _)| *a).collect();
        attrs.sort();
        prop_assert_eq!(
            attrs,
            vec![
                "buffer/byte-size",
                "buffer/dirty",
                "buffer/observable",
                "buffer/path",
                "buffer/version",
            ],
        );

        for (attribute, value) in &facts {
            // (2) Variant discipline: only String, U64, Bool reachable.
            prop_assert!(
                matches!(value, FactValue::String(_) | FactValue::U64(_) | FactValue::Bool(_)),
                "fact {attribute} has disallowed value variant {value:?}",
            );

            // (3) Attribute→type map pinned.
            match *attribute {
                "buffer/path" => {
                    let FactValue::String(s) = value else {
                        prop_assert!(false, "buffer/path must be a String");
                        continue;
                    };
                    let expected = canonical.display().to_string();
                    prop_assert_eq!(s, &expected);

                    // (4) No content leakage: the String value cannot be
                    // the file's content verbatim. A literal equality
                    // check is the sharpest signal the discipline is
                    // intact — a canonical path and the file's bytes
                    // can only collide pathologically (path-shaped
                    // content on a path-shaped filesystem), and any
                    // drift from path→content would be caught here.
                    prop_assert_ne!(
                        s.as_bytes(),
                        content.as_slice(),
                        "buffer/path must not echo file content verbatim (FR-002a)",
                    );
                }
                "buffer/byte-size" => {
                    let FactValue::U64(n) = value else {
                        prop_assert!(false, "buffer/byte-size must be a U64");
                        continue;
                    };
                    prop_assert_eq!(*n, content.len() as u64);
                }
                "buffer/dirty" => {
                    let FactValue::Bool(b) = value else {
                        prop_assert!(false, "buffer/dirty must be a Bool");
                        continue;
                    };
                    prop_assert!(!*b, "bootstrap buffer/dirty is always false");
                }
                "buffer/observable" => {
                    let FactValue::Bool(b) = value else {
                        prop_assert!(false, "buffer/observable must be a Bool");
                        continue;
                    };
                    prop_assert!(*b, "bootstrap buffer/observable is always true");
                }
                "buffer/version" => {
                    let FactValue::U64(n) = value else {
                        prop_assert!(false, "buffer/version must be a U64");
                        continue;
                    };
                    prop_assert_eq!(*n, 0u64, "bootstrap buffer/version is always 0");
                }
                other => prop_assert!(false, "unknown attribute {other}"),
            }
        }
    }
}
