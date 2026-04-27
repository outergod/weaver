//! T017 — property test for `BufferState::apply_edits` validation as an
//! iff over the data-model rules, plus the two structural postconditions.
//!
//! Covers `specs/004-buffer-edit/data-model.md §Validation rules`
//! (R1..R6 plus intra-batch overlap) under random ASCII content and
//! random edit batches. The test asserts:
//!
//!   1. `apply_edits.is_ok() ↔ batch satisfies R1, R2, R3, R5, no
//!      intra-batch overlap`. R4 (mid-codepoint) is structurally
//!      side-stepped here — content is constrained to ASCII so every
//!      `character` index is a codepoint boundary by construction. R6
//!      (invalid-utf8) is structurally unreachable under safe Rust per
//!      the unit tests in `buffers/src/model.rs::tests`. Both are
//!      deliberately out-of-scope for the iff so the proptest doesn't
//!      have to reimplement UTF-8 boundary detection redundantly with
//!      `LineIndex`.
//!   2. On `Ok`, `state.memory_digest == sha256(state.content())`
//!      (the slice-003 invariant).
//!   3. On `Err`, `state.content` and `state.memory_digest` are
//!      byte-identical to their pre-apply values (atomicity).
//!
//! The test mirrors R1/R2/R3/R5 + overlap with an independent O(n²)
//! all-pairs implementation. The cross-check is informative: when the
//! impl's sort-and-linear-scan and the test's all-pairs scan agree
//! across 256 random cases per run, both are likely correct.

use std::io::Write;

use proptest::prelude::*;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use weaver_buffers::model::BufferState;
use weaver_core::types::edit::{Position, Range, TextEdit};

/// ASCII printable + newline. Bounded 0..=256 bytes so the proptest
/// runtime stays modest.
fn arb_content() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(prop_oneof![Just(b'\n'), 0x20u8..=0x7Eu8], 0..=256)
}

fn arb_position(max_line: u32, max_char: u32) -> impl Strategy<Value = Position> {
    (0..=max_line, 0..=max_char).prop_map(|(line, character)| Position { line, character })
}

fn arb_text_edit() -> impl Strategy<Value = TextEdit> {
    // The line/char bounds straddle "valid" and "out-of-bounds" so the
    // generator covers both branches of the iff. `new_text` is ASCII
    // only — UTF-8 boundary mechanics are unit-tested elsewhere.
    (
        arb_position(8, 32),
        arb_position(8, 32),
        proptest::collection::vec(0x20u8..=0x7Eu8, 0..=8),
    )
        .prop_map(|(start, end, txt)| TextEdit {
            range: Range { start, end },
            new_text: String::from_utf8(txt).expect("ascii-only by construction"),
        })
}

fn arb_batch() -> impl Strategy<Value = Vec<TextEdit>> {
    proptest::collection::vec(arb_text_edit(), 0..=8)
}

fn state_from_bytes(content: &[u8]) -> BufferState {
    let mut f = NamedTempFile::new().expect("tempfile");
    f.write_all(content).expect("write");
    f.flush().expect("flush");
    let canonical = std::fs::canonicalize(f.path()).expect("canonicalize");
    BufferState::open(canonical).expect("open")
}

/// Independent validity oracle. ASCII-only assumption keeps R4 trivial;
/// the function checks R1, R2, R3, R5, and intra-batch overlap with an
/// algorithm structurally distinct from `apply_edits`'s sort-and-scan.
///
/// Returns `Ok(())` iff the batch is structurally valid by R1+R2+R3+R5
/// and has no intra-batch overlap.
fn classify_batch(content: &[u8], edits: &[TextEdit]) -> Result<(), &'static str> {
    if edits.is_empty() {
        return Ok(());
    }

    // Build line metadata using the same `\n`-separator + drop-trailing-
    // newline rule as `LineIndex`.
    let mut line_starts: Vec<usize> = vec![0];
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    if matches!(content.last(), Some(&b'\n')) {
        line_starts.pop();
    }
    let line_count = line_starts.len() as u32;

    let line_byte_length = |line: u32| -> usize {
        let line_idx = line as usize;
        let start = line_starts[line_idx];
        if line_idx + 1 < line_starts.len() {
            line_starts[line_idx + 1] - 1 - start
        } else if matches!(content.last(), Some(&b'\n')) {
            content.len() - 1 - start
        } else {
            content.len() - start
        }
    };

    let position_in_bounds = |p: Position| -> bool {
        if p.line < line_count {
            (p.character as usize) <= line_byte_length(p.line)
        } else {
            // Past-last virtual position is in-bounds only at (line_count, 0).
            p.line == line_count && p.character == 0
        }
    };

    // Per-edit checks (R1, R5, R2/R3). R4 trivially holds under ASCII.
    for edit in edits {
        if edit.range.start > edit.range.end {
            return Err("swapped-endpoints");
        }
        if edit.range.start == edit.range.end && edit.new_text.is_empty() {
            return Err("nothing-edit");
        }
        if !position_in_bounds(edit.range.start) || !position_in_bounds(edit.range.end) {
            return Err("out-of-bounds");
        }
    }

    // O(n²) all-pairs overlap. Mirror data-model rule: tied starts where
    // both are pure inserts are NOT overlap; otherwise any (start, end)
    // intersection is.
    for i in 0..edits.len() {
        for j in (i + 1)..edits.len() {
            let a = &edits[i].range;
            let b = &edits[j].range;
            let a_is_insert = a.start == a.end;
            let b_is_insert = b.start == b.end;
            let (lo, hi) = if a.start <= b.start { (a, b) } else { (b, a) };
            let strictly_overlaps = lo.end > hi.start;
            let tied_starts = a.start == b.start;
            let tied_with_non_insert = tied_starts && !(a_is_insert && b_is_insert);
            if strictly_overlaps || tied_with_non_insert {
                return Err("intra-batch-overlap");
            }
        }
    }

    Ok(())
}

proptest! {
    /// The headline iff: `apply_edits` accepts iff the batch is
    /// structurally valid by the independent oracle. The two
    /// postconditions ride on the same case so we get them under all
    /// generators.
    #[test]
    fn apply_edits_accepts_iff_batch_is_structurally_valid(
        content in arb_content(),
        batch in arb_batch(),
    ) {
        let mut s = state_from_bytes(&content);
        let content_before = s.content().to_vec();
        let digest_before = *s.memory_digest();

        let oracle = classify_batch(&content, &batch);
        let result = s.apply_edits(&batch);

        prop_assert_eq!(
            oracle.is_ok(),
            result.is_ok(),
            "validity oracle disagrees with apply_edits for content={:?}, batch={:?}, oracle={:?}, result={:?}",
            content_before,
            batch,
            oracle,
            result,
        );

        match result {
            Ok(()) => {
                let expected: [u8; 32] = Sha256::digest(s.content()).into();
                prop_assert_eq!(
                    s.memory_digest(),
                    &expected,
                    "memory_digest invariant broken on accept",
                );
            }
            Err(_) => {
                prop_assert_eq!(
                    s.content(),
                    content_before.as_slice(),
                    "content mutated despite rejection",
                );
                prop_assert_eq!(
                    s.memory_digest(),
                    &digest_before,
                    "memory_digest mutated despite rejection",
                );
            }
        }
    }

    /// FR-008: empty batch is always accepted as a structural identity.
    /// Pinned independently so the iff property's iff would be verified
    /// even if its oracle had a bug in the empty-batch arm.
    #[test]
    fn empty_batch_is_always_accepted_as_identity(content in arb_content()) {
        let mut s = state_from_bytes(&content);
        let content_before = s.content().to_vec();
        let digest_before = *s.memory_digest();
        s.apply_edits(&[]).expect("empty batch must be Ok");
        prop_assert_eq!(s.content(), content_before.as_slice());
        prop_assert_eq!(s.memory_digest(), &digest_before);
    }
}
