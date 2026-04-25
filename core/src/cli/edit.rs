//! `weaver edit` and `weaver edit-json` subcommand implementation.
//!
//! Slice 004 ships the positional `weaver edit` form. The JSON form
//! (`weaver edit-json`) lands in slice 004's US3 phase.
//!
//! See `specs/004-buffer-edit/contracts/cli-surfaces.md`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use miette::{IntoDiagnostic, miette};
use thiserror::Error;
use tokio::runtime::Builder;
use tracing::warn;

use crate::bus::client::{Client, ClientError};
use crate::cli::args::OutputFormat;
use crate::cli::config::Config;
use crate::cli::errors::{WeaverCliError, render_error};
use crate::provenance::{ActorIdentity, Provenance};
use crate::types::buffer_entity::buffer_entity_ref;
use crate::types::edit::{Position, Range, TextEdit};
use crate::types::event::{Event, EventPayload};
use crate::types::fact::{FactKey, FactValue};
use crate::types::ids::EventId;
use crate::types::message::{BusMessage, InspectionError};

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

/// Convert a flat `Vec<String>` of `[<RANGE>, <TEXT>, <RANGE>, <TEXT>, ...]`
/// into a `Vec<TextEdit>`. Validates pair-count parity AND range
/// shape. Surfaces failures as [`WeaverCliError::InvalidRange`] with
/// the offending input + detail string for WEAVER-EDIT-002 rendering.
///
/// Empty input → empty Vec (the zero-pair "no edits provided" case is
/// handled higher up in [`handle_edit`] before parsing).
pub fn parse_pairs(pairs: &[String]) -> Result<Vec<TextEdit>, WeaverCliError> {
    if pairs.len() % 2 != 0 {
        return Err(WeaverCliError::InvalidRange {
            input: pairs.last().cloned().unwrap_or_default(),
            detail: format!(
                "expected an even number of <RANGE> <TEXT> arguments, got {}",
                pairs.len()
            ),
            context: Some("weaver edit".into()),
        });
    }
    let mut edits = Vec::with_capacity(pairs.len() / 2);
    for chunk in pairs.chunks_exact(2) {
        let range_str = &chunk[0];
        let new_text = chunk[1].clone();
        let range = parse_range(range_str).map_err(|e| WeaverCliError::InvalidRange {
            input: range_str.clone(),
            detail: e.to_string(),
            context: Some("weaver edit".into()),
        })?;
        edits.push(TextEdit { range, new_text });
    }
    Ok(edits)
}

/// Run `weaver edit <PATH> [<RANGE> <TEXT>]*` end-to-end.
///
/// Flow per `specs/004-buffer-edit/contracts/cli-surfaces.md
/// §weaver edit §Pre-dispatch flow`:
///
/// 1. Zero-pair invocation → warn-stderr + exit 0 (FR-014).
/// 2. Parse pairs into `Vec<TextEdit>`; on failure render
///    WEAVER-EDIT-002 (exit 1).
/// 3. Canonicalise path; on failure render parse-error (exit 1).
/// 4. Derive `entity = buffer_entity_ref(canonical)`.
/// 5. Connect to bus.
/// 6. Inspect-lookup `(entity, buffer/version)`:
///    - `FactNotFound` → render WEAVER-EDIT-001 (exit 1).
///    - `Found` with `value: FactValue::U64(version)` → use `version`.
///    - Other shapes → exit 10 (constitutional violation).
/// 7. Construct `Event { payload: BufferEdit { entity, version, edits } }`
///    with `Provenance { source: ActorIdentity::User, .. }`.
/// 8. Dispatch via `BusMessage::Event`; close + exit 0
///    (fire-and-forget per FR-012).
pub fn handle_edit(
    path: PathBuf,
    pairs: Vec<String>,
    output: OutputFormat,
    socket_override: Option<PathBuf>,
) -> miette::Result<()> {
    // Step 1: zero-pair → warn + exit 0 (FR-014).
    if pairs.is_empty() {
        warn!("no edits provided; nothing dispatched");
        eprintln!("weaver edit: no edits provided; nothing dispatched");
        return Ok(());
    }

    // Step 2: parse pairs.
    let edits = match parse_pairs(&pairs) {
        Ok(e) => e,
        Err(err) => {
            render_error(&err, output)?;
            std::process::exit(err.exit_code());
        }
    };

    // Step 3: canonicalise path.
    let canonical = match std::fs::canonicalize(&path) {
        Ok(c) => c,
        Err(source) => {
            let err = WeaverCliError::ParseError {
                message: format!("cannot canonicalise path {}: {source}", path.display()),
                context: Some(format!("weaver edit {}", path.display())),
            };
            render_error(&err, output)?;
            std::process::exit(err.exit_code());
        }
    };

    // Step 4: derive entity.
    let entity = buffer_entity_ref(&canonical);

    // Steps 5-8: bus interaction.
    let cfg = Config::from_cli(socket_override);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;
    runtime.block_on(async move {
        let mut client = match Client::connect(&cfg.socket_path, "weaver-edit").await {
            Ok(c) => c,
            Err(ClientError::Connect {
                path: socket_path,
                source,
            }) => {
                let err = WeaverCliError::CoreUnavailable {
                    message: format!("core not reachable at {socket_path}: {source}"),
                    context: Some("weaver edit".into()),
                };
                render_error(&err, output)?;
                std::process::exit(err.exit_code());
            }
            Err(e) => return Err(miette!("{e}")),
        };

        // Step 6: inspect-lookup buffer/version.
        let key = FactKey::new(entity, "buffer/version");
        let request_id = next_request_id();
        client
            .send(&BusMessage::InspectRequest {
                request_id,
                fact: key.clone(),
            })
            .await
            .map_err(|e| miette!("{e}"))?;
        let response = loop {
            match client.recv().await.map_err(|e| miette!("{e}"))? {
                BusMessage::InspectResponse {
                    request_id: rid,
                    result,
                } if rid == request_id => break result,
                // Defensive: ignore spurious frames from a non-
                // subscribed connection. Should not happen.
                _ => continue,
            }
        };
        let version = match response {
            Err(InspectionError::FactNotFound) => {
                let err = WeaverCliError::BufferNotOpened {
                    path: canonical.display().to_string(),
                    entity: entity.as_u64(),
                    context: Some(format!("weaver edit {}", canonical.display())),
                };
                render_error(&err, output)?;
                std::process::exit(err.exit_code());
            }
            Err(InspectionError::NoProvenance) => {
                let err = WeaverCliError::ProtocolError {
                    message: format!(
                        "buffer/version exists for entity {} but has no provenance",
                        entity.as_u64()
                    ),
                    context: Some("weaver edit inspect-lookup".into()),
                };
                render_error(&err, output)?;
                std::process::exit(10);
            }
            Ok(detail) => match detail.value {
                FactValue::U64(v) => v,
                other => {
                    let err = WeaverCliError::ProtocolError {
                        message: format!(
                            "buffer/version (entity {}) expected U64 but got {other:?}",
                            entity.as_u64()
                        ),
                        context: Some("weaver edit inspect-lookup".into()),
                    };
                    render_error(&err, output)?;
                    std::process::exit(10);
                }
            },
        };

        // Step 7: construct event envelope. Provenance carries
        // ActorIdentity::User per research §6 — this is the first
        // production use of the variant reserved at slice 002.
        // EventId is synthesised from wall-clock ns; uniqueness
        // within a single CLI invocation is enough (the trace
        // dedupes via stable ordering, not via id collisions).
        let now = now_ns();
        let provenance = Provenance::new(ActorIdentity::User, now, None)
            .expect("ActorIdentity::User has no fields to validate");
        let event = Event {
            id: EventId::new(now),
            name: "buffer/edit".into(),
            target: Some(entity),
            payload: EventPayload::BufferEdit {
                entity,
                version,
                edits,
            },
            provenance,
        };

        // Step 8: dispatch + exit 0. Drop closes the connection
        // gracefully — the kernel flushes the queued Event to the
        // listener before SHUT_WR is observed.
        client
            .send(&BusMessage::Event(event))
            .await
            .map_err(|e| miette!("{e}"))?;
        Ok(())
    })
}

/// `weaver edit-json <PATH> --from <PATH-or-dash>` handler stub.
///
/// T019 lands the grammar; T020 replaces this body with the JSON-read,
/// parse, size-check, and dispatch flow that reuses the canonicalise,
/// inspect-lookup, and envelope-construction path from `handle_edit`.
/// Until then this returns a `not yet wired` parse-error so the
/// integration is build-green and `weaver edit-json` exits 1 with a
/// self-explaining diagnostic if invoked.
pub fn handle_edit_json(
    _path: PathBuf,
    _from: PathBuf,
    output: OutputFormat,
    _socket_override: Option<PathBuf>,
) -> miette::Result<()> {
    let err = WeaverCliError::ParseError {
        message: "weaver edit-json: handler lands in T020 (Phase 5 US3)".into(),
        context: Some("weaver edit-json".into()),
    };
    render_error(&err, output)?;
    std::process::exit(err.exit_code());
}

/// Monotonic per-process inspect request-id counter. One
/// `weaver edit` invocation issues exactly one InspectRequest, but the
/// counter is process-scoped to keep the shape consistent with
/// `cli::inspect::next_request_id`.
fn next_request_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
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

    // ───────────────────────────────────────────────────────────────────
    // T013: parse_pairs handler-helper tests. The full handle_edit
    // dispatch path (canonicalise → connect → inspect → dispatch) is
    // covered by tests/e2e/buffer_edit_single.rs since it requires a
    // running core + buffer-service; here we cover the pure-logic
    // parsing seam in isolation.
    // ───────────────────────────────────────────────────────────────────

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn parse_pairs_empty_returns_empty() {
        let out = parse_pairs(&[]).expect("empty input is Ok");
        assert!(out.is_empty());
    }

    #[test]
    fn parse_pairs_single_valid_pair() {
        let out = parse_pairs(&[s("0:0-0:0"), s("hello ")]).expect("ok");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].new_text, "hello ");
        assert_eq!(out[0].range.start, pos(0, 0));
        assert_eq!(out[0].range.end, pos(0, 0));
    }

    #[test]
    fn parse_pairs_three_valid_pairs() {
        let out = parse_pairs(&[
            s("0:0-0:0"),
            s("A"),
            s("1:0-1:0"),
            s("B"),
            s("2:0-2:0"),
            s("C"),
        ])
        .expect("ok");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].new_text, "A");
        assert_eq!(out[1].new_text, "B");
        assert_eq!(out[2].new_text, "C");
    }

    #[test]
    fn parse_pairs_odd_count_rejects_with_invalid_range() {
        let err = parse_pairs(&[s("0:0-0:0"), s("A"), s("1:0-1:0")]).expect_err("odd count");
        match err {
            WeaverCliError::InvalidRange { detail, .. } => {
                assert!(detail.contains("even number"), "got: {detail}");
            }
            other => panic!("expected InvalidRange, got {other:?}"),
        }
    }

    #[test]
    fn parse_pairs_bad_range_in_middle_rejects() {
        let err = parse_pairs(&[
            s("0:0-0:0"),
            s("A"),
            s("not-a-range"),
            s("B"),
            s("2:0-2:0"),
            s("C"),
        ])
        .expect_err("bad range");
        match err {
            WeaverCliError::InvalidRange { input, .. } => {
                assert_eq!(input, "not-a-range");
            }
            other => panic!("expected InvalidRange, got {other:?}"),
        }
    }
}
