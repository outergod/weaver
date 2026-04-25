//! Text-edit primitives: `Position`, `Range`, `TextEdit`.
//!
//! Slice-004 wire types. See `specs/004-buffer-edit/data-model.md`
//! and `specs/004-buffer-edit/contracts/bus-messages.md`.
//!
//! Coordinate system is LSP-style with one deliberate departure:
//! `Position.character` counts **UTF-8 bytes within the line's
//! content**, NOT UTF-16 code units. LSP 3.17's `positionEncodings`
//! capability negotiation makes UTF-8 first-class, so this stays
//! interoperable with future LSP wiring.
//!
//! These types are plain structs (no enum tagging); they serialise as
//! CBOR maps / JSON objects. `TextEdit` carries a kebab-case wire
//! rename for `new_text` → `new-text` per Amendment 5.

use serde::{Deserialize, Serialize};

/// A 2D coordinate within a buffer.
///
/// - `line` — zero-based line index. Lines are separated by `\n`;
///   `\r\n` keeps the `\r` as the last byte of the preceding line.
/// - `character` — zero-based UTF-8 byte offset within the line's
///   content. Endpoints landing mid-codepoint (inside a multi-byte
///   UTF-8 sequence) are rejected at validation.
///
/// Derived `Ord` / `PartialOrd` give lexicographic comparison on
/// `(line, character)` (Rust derives field-order ordering); the
/// publisher's apply-edits pipeline relies on this for `start <= end`
/// validation and for sort-by-start ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A half-open interval on 2D buffer coordinates: inclusive `start`,
/// exclusive `end`. Point ranges (`start == end`) represent insertion
/// cursors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// One atomic edit operation: replace the bytes within `range` by
/// `new_text.as_bytes()`.
///
/// The Rust field name is `new_text` (snake_case per Amendment 5
/// in-language idiom); the JSON field name is `new-text` (kebab-case
/// per Amendment 5 wire idiom).
///
/// - **Pure-insert**: `range.start == range.end && new_text != ""`.
/// - **Pure-delete**: `range.start < range.end && new_text == ""`.
/// - **Nothing-edit (rejected)**: `range.start == range.end &&
///   new_text == ""`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn rng(start: Position, end: Position) -> Range {
        Range { start, end }
    }

    #[test]
    fn position_json_round_trip() {
        let p = pos(42, 12);
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(s, r#"{"line":42,"character":12}"#);
        let back: Position = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn range_json_round_trip() {
        let r = rng(pos(0, 0), pos(0, 5));
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(
            s,
            r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":5}}"#
        );
        let back: Range = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn text_edit_json_round_trip_uses_kebab_case_new_text() {
        let e = TextEdit {
            range: rng(pos(0, 0), pos(0, 0)),
            new_text: "hello ".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        // Wire field is `new-text` (kebab-case) per Amendment 5.
        assert!(
            s.contains("\"new-text\":\"hello \""),
            "expected kebab-case `new-text` field: {s}"
        );
        assert!(
            !s.contains("new_text"),
            "snake_case must not leak to the wire: {s}"
        );
        let back: TextEdit = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn text_edit_cbor_round_trip() {
        let e = TextEdit {
            range: rng(pos(3, 4), pos(7, 0)),
            new_text: "βγ".into(),
        };
        let mut buf = Vec::new();
        ciborium::into_writer(&e, &mut buf).unwrap();
        let back: TextEdit = ciborium::from_reader(buf.as_slice()).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn position_ordering_is_lexicographic_on_line_then_character() {
        // Within the same line, character orders.
        assert!(pos(0, 0) < pos(0, 1));
        assert!(pos(0, 5) < pos(0, 6));
        // Line dominates character.
        assert!(pos(0, 999) < pos(1, 0));
        assert!(pos(7, 0) > pos(6, 999_999));
        // Equality is reflexive.
        assert_eq!(pos(4, 4), pos(4, 4));
    }

    /// Documents the contract that the apply-edits validator relies on:
    /// `Position.character` MUST land on a UTF-8 codepoint boundary
    /// within the line's bytes. The validator rejects mid-codepoint
    /// endpoints via `str::is_char_boundary`. This test pins the
    /// behaviour we depend on (Rust stdlib semantics).
    #[test]
    fn is_char_boundary_pins_validator_contract() {
        // "𝔸" is U+1D538 — 4 UTF-8 bytes: F0 9D 94 B8.
        let line = "𝔸";
        // Boundary at 0 (start) and 4 (end of codepoint) is valid.
        assert!(line.is_char_boundary(0));
        assert!(line.is_char_boundary(4));
        // Bytes 1..=3 are mid-codepoint and would be rejected as
        // R4 `MidCodepointBoundary` if used as `Position.character`.
        assert!(!line.is_char_boundary(1));
        assert!(!line.is_char_boundary(2));
        assert!(!line.is_char_boundary(3));

        // Mixed ASCII + multi-byte: "héllo" — h=1, é=2, llo=3.
        let line = "héllo";
        assert!(line.is_char_boundary(0)); // start
        assert!(line.is_char_boundary(1)); // before é
        assert!(!line.is_char_boundary(2)); // mid é
        assert!(line.is_char_boundary(3)); // after é
        assert!(line.is_char_boundary(line.len())); // end
    }
}
