//! `weaver edit` and `weaver edit-json` subcommand implementation.
//!
//! Slice 004 ships the positional `weaver edit` form. The JSON form
//! (`weaver edit-json`) lands in slice 004's US3 phase.
//!
//! See `specs/004-buffer-edit/contracts/cli-surfaces.md`.

use thiserror::Error;

use crate::types::edit::{Position, Range};

/// Failure modes for [`parse_range`]. Each variant carries the offending
/// input so the caller can render WEAVER-EDIT-002 with a precise
/// diagnostic body.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RangeParseError {
    /// The string did not match `<sl>:<sc>-<el>:<ec>` shape.
    #[error("invalid range \"{input}\": expected <start-line>:<start-char>-<end-line>:<end-char>")]
    Format { input: String },
    /// One of the four `u32` components failed to parse (overflow,
    /// non-decimal, or non-numeric).
    #[error("invalid range \"{input}\": {component} is not a decimal u32 in range [0, 2^32)")]
    Component { input: String, component: String },
}

/// Parse a `<sl>:<sc>-<el>:<ec>` range string into a [`Range`].
///
/// `sl`/`sc`/`el`/`ec` are decimal `u32` values. The character offsets
/// are UTF-8 byte offsets within the line's content, per
/// `contracts/bus-messages.md §Position`.
///
/// Examples:
/// - `"0:0-0:0"` → point cursor at start of buffer.
/// - `"0:0-0:5"` → first 5 bytes of line 0.
/// - `"2:10-3:0"` → line 2 byte 10 through end of line 2 (exclusive).
pub fn parse_range(input: &str) -> Result<Range, RangeParseError> {
    let (start_str, end_str) = input
        .split_once('-')
        .ok_or_else(|| RangeParseError::Format {
            input: input.to_string(),
        })?;
    let start = parse_position(start_str, input, "start")?;
    let end = parse_position(end_str, input, "end")?;
    Ok(Range { start, end })
}

fn parse_position(
    pos_str: &str,
    full_input: &str,
    side: &'static str,
) -> Result<Position, RangeParseError> {
    let (line_str, char_str) = pos_str
        .split_once(':')
        .ok_or_else(|| RangeParseError::Format {
            input: full_input.to_string(),
        })?;
    let line = line_str
        .parse::<u32>()
        .map_err(|_| RangeParseError::Component {
            input: full_input.to_string(),
            component: format!("{side}-line \"{line_str}\""),
        })?;
    let character = char_str
        .parse::<u32>()
        .map_err(|_| RangeParseError::Component {
            input: full_input.to_string(),
            component: format!("{side}-character \"{char_str}\""),
        })?;
    Ok(Position { line, character })
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
    fn parses_zero_zero_zero_zero_point_cursor() {
        assert_eq!(parse_range("0:0-0:0").unwrap(), rng(pos(0, 0), pos(0, 0)));
    }

    #[test]
    fn parses_first_five_bytes_of_line_zero() {
        assert_eq!(parse_range("0:0-0:5").unwrap(), rng(pos(0, 0), pos(0, 5)));
    }

    #[test]
    fn parses_multi_line_range() {
        assert_eq!(parse_range("2:10-3:0").unwrap(), rng(pos(2, 10), pos(3, 0)));
    }

    #[test]
    fn parses_max_u32_components() {
        let max = u32::MAX;
        let s = format!("{max}:{max}-{max}:{max}");
        assert_eq!(parse_range(&s).unwrap(), rng(pos(max, max), pos(max, max)));
    }

    #[test]
    fn rejects_missing_dash() {
        let err = parse_range("0:0").unwrap_err();
        assert!(matches!(err, RangeParseError::Format { .. }));
    }

    #[test]
    fn rejects_missing_colon_in_start() {
        let err = parse_range("0-0:5").unwrap_err();
        assert!(matches!(err, RangeParseError::Format { .. }));
    }

    #[test]
    fn rejects_missing_colon_in_end() {
        let err = parse_range("0:0-5").unwrap_err();
        assert!(matches!(err, RangeParseError::Format { .. }));
    }

    #[test]
    fn rejects_negative_components() {
        // u32 parse rejects leading `-`. The split on `-` happens first,
        // so `"0:0--1:5"` → start=`"0:0"`, end=`"-1:5"` → end-line `-1`
        // fails u32 parse and surfaces as a Component error.
        match parse_range("0:0--1:5").unwrap_err() {
            RangeParseError::Component { component, .. } => {
                assert!(component.contains("end-line"), "got: {component}");
            }
            other => panic!("expected Component, got {other:?}"),
        }
    }

    #[test]
    fn rejects_non_decimal_components() {
        let err = parse_range("0xff:0-0:0").unwrap_err();
        assert!(matches!(err, RangeParseError::Component { .. }));
    }

    #[test]
    fn rejects_overflow_beyond_u32_max() {
        // u32::MAX + 1 = 4294967296.
        let err = parse_range("4294967296:0-0:0").unwrap_err();
        assert!(matches!(err, RangeParseError::Component { .. }));
    }

    #[test]
    fn rejects_empty_string() {
        let err = parse_range("").unwrap_err();
        assert!(matches!(err, RangeParseError::Format { .. }));
    }

    #[test]
    fn rejects_spurious_extra_characters_in_end_component() {
        // split_once('-') gives start="0:0", end="0:5-extra"; the end
        // half then split on ':' gives line="0", character="5-extra"
        // → character parse fails on the trailing "-extra".
        let err = parse_range("0:0-0:5-extra").unwrap_err();
        assert!(matches!(err, RangeParseError::Component { .. }));
    }
}
