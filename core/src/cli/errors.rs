//! CLI error envelope for L2 P6 (humane shell) — typed errors with both
//! human and JSON rendering.
//!
//! Human rendering delegates to [`miette`] (fancy output by default).
//! JSON rendering produces the shape documented in
//! `specs/001-hello-fact/contracts/cli-surfaces.md`:
//!
//! ```json
//! {
//!   "error": {
//!     "category": "core-unavailable",
//!     "code": "WEAVER-002",
//!     "message": "...",
//!     "context": "...",
//!     "fact_key": null
//!   }
//! }
//! ```
//!
//! Slice 001 wires up the codes the Hello-fact CLI actually surfaces
//! (`core-unavailable`, `fact-not-found`, `parse-error`,
//! `protocol-error`). Additional codes land with their first surface.

use miette::{Diagnostic, IntoDiagnostic};
use serde::Serialize;
use thiserror::Error;

use crate::cli::args::OutputFormat;
use crate::types::fact::FactKey;

/// Exit-code convention shared by the CLI subcommands.
pub mod exit_code {
    pub const OK: i32 = 0;
    /// `miette::Report` default (unused directly — kept for completeness).
    pub const GENERAL: i32 = 1;
    /// "Expected" error classes: core unreachable, fact not found.
    pub const EXPECTED: i32 = 2;
}

/// Structured shape surfaced by `-o json` when an error occurs.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub category: &'static str,
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub fact_key: Option<FactKeyJson>,
}

#[derive(Debug, Serialize)]
pub struct FactKeyJson {
    pub entity: u64,
    pub attribute: String,
}

impl FactKeyJson {
    pub fn from(key: &FactKey) -> Self {
        Self {
            entity: key.entity.as_u64(),
            attribute: key.attribute.clone(),
        }
    }
}

/// CLI-level typed errors. Each variant carries the metadata needed to
/// render both human (via `miette`) and JSON (via `ErrorEnvelope`)
/// forms.
#[derive(Debug, Error, Diagnostic)]
pub enum WeaverCliError {
    #[error("{message}")]
    #[diagnostic(code("WEAVER-002"))]
    CoreUnavailable {
        message: String,
        context: Option<String>,
    },

    #[error("fact not found: {key}")]
    #[diagnostic(code("WEAVER-201"))]
    FactNotFound {
        key: FactKey,
        context: Option<String>,
    },

    #[error("invalid input: {message}")]
    #[diagnostic(code("WEAVER-101"))]
    ParseError {
        message: String,
        context: Option<String>,
    },

    #[error("bus protocol error: {message}")]
    #[diagnostic(code("WEAVER-301"))]
    ProtocolError {
        message: String,
        context: Option<String>,
    },

    /// WEAVER-EDIT-001 — the target buffer is not opened by any
    /// `weaver-buffers` instance on the bus. Surfaced when
    /// `weaver edit`'s pre-dispatch inspect-lookup of `buffer/version`
    /// returns `FactNotFound`. Exit 1.
    #[error(
        "buffer not opened: {path} — no fact (entity:{entity}, attribute:buffer/version) is asserted by any authority. Run `weaver-buffers {path}` to open the buffer."
    )]
    #[diagnostic(code("WEAVER-EDIT-001"))]
    BufferNotOpened {
        path: String,
        entity: u64,
        context: Option<String>,
    },

    /// WEAVER-EDIT-002 — `weaver edit`'s positional `<RANGE>` argument
    /// did not parse as `<sl>:<sc>-<el>:<ec>`, OR the variadic
    /// `<RANGE> <TEXT>` pairs had an odd element count. Exit 1.
    #[error("invalid range \"{input}\": {detail}")]
    #[diagnostic(code("WEAVER-EDIT-002"))]
    InvalidRange {
        input: String,
        detail: String,
        context: Option<String>,
    },

    /// WEAVER-EDIT-003 — `weaver edit-json` could not parse its input
    /// as a JSON `Vec<TextEdit>`. The `detail` field carries the
    /// serde-json error chain so the operator can locate the offending
    /// span. Exit 1.
    #[error("malformed edit-json input: {detail}")]
    #[diagnostic(code("WEAVER-EDIT-003"))]
    MalformedEditJson {
        detail: String,
        context: Option<String>,
    },

    /// WEAVER-EDIT-004 — the serialised `EventPayload::BufferEdit`
    /// envelope exceeds the ingest-frame limit. The limit is smaller
    /// than the wire-level `MAX_FRAME_SIZE` (64 KiB) by
    /// `RESPONSE_WRAPPER_HEADROOM` so the same `Event`, when wrapped as
    /// `BusMessage::EventInspectResponse` during `weaver inspect --why`,
    /// still fits within `MAX_FRAME_SIZE` on the response side. The
    /// serialised byte count is captured pre-dispatch so the operator
    /// gets a precise diagnostic rather than a generic codec error
    /// after a partial round-trip. Exit 1.
    #[error(
        "serialised BufferEdit ({actual_bytes} bytes) exceeds ingest-frame limit ({max_bytes} bytes). Reduce the batch size or shorten new-text fields."
    )]
    #[diagnostic(code("WEAVER-EDIT-004"))]
    EditWireFrameTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
        context: Option<String>,
    },

    /// WEAVER-SAVE-001 — the target buffer is not opened by any
    /// `weaver-buffers` instance on the bus. Surfaced when
    /// `weaver save`'s pre-dispatch inspect-lookup of `buffer/version`
    /// returns `FactNotFound`. Same situation as `BufferNotOpened`
    /// (WEAVER-EDIT-001) on a different surface; the code-per-surface
    /// split lets operators grep traces by code without conflating
    /// the edit-side and save-side instances. Exit 1.
    #[error(
        "buffer not opened: {entity_arg} — no fact (entity:{entity}, attribute:buffer/version) is asserted by any authority. Run `weaver-buffers <PATH>` to open the buffer."
    )]
    #[diagnostic(code("WEAVER-SAVE-001"))]
    BufferNotOpenedSave {
        entity_arg: String,
        entity: u64,
        context: Option<String>,
    },
}

impl WeaverCliError {
    /// One-line category string surfaced in both human and JSON modes.
    pub fn category(&self) -> &'static str {
        match self {
            WeaverCliError::CoreUnavailable { .. } => "core-unavailable",
            WeaverCliError::FactNotFound { .. } => "fact-not-found",
            WeaverCliError::ParseError { .. } => "parse-error",
            WeaverCliError::ProtocolError { .. } => "protocol-error",
            WeaverCliError::BufferNotOpened { .. } => "buffer-not-opened",
            WeaverCliError::InvalidRange { .. } => "invalid-range",
            WeaverCliError::MalformedEditJson { .. } => "malformed-edit-json",
            WeaverCliError::EditWireFrameTooLarge { .. } => "edit-wire-frame-too-large",
            WeaverCliError::BufferNotOpenedSave { .. } => "buffer-not-opened",
        }
    }

    pub fn code_str(&self) -> &'static str {
        // Mirrors the `#[diagnostic(code(...))]` above so the JSON form
        // can emit it without reaching into miette's trait machinery.
        match self {
            WeaverCliError::CoreUnavailable { .. } => "WEAVER-002",
            WeaverCliError::FactNotFound { .. } => "WEAVER-201",
            WeaverCliError::ParseError { .. } => "WEAVER-101",
            WeaverCliError::ProtocolError { .. } => "WEAVER-301",
            WeaverCliError::BufferNotOpened { .. } => "WEAVER-EDIT-001",
            WeaverCliError::InvalidRange { .. } => "WEAVER-EDIT-002",
            WeaverCliError::MalformedEditJson { .. } => "WEAVER-EDIT-003",
            WeaverCliError::EditWireFrameTooLarge { .. } => "WEAVER-EDIT-004",
            WeaverCliError::BufferNotOpenedSave { .. } => "WEAVER-SAVE-001",
        }
    }

    pub fn context(&self) -> Option<&str> {
        match self {
            WeaverCliError::CoreUnavailable { context, .. }
            | WeaverCliError::FactNotFound { context, .. }
            | WeaverCliError::ParseError { context, .. }
            | WeaverCliError::ProtocolError { context, .. }
            | WeaverCliError::BufferNotOpened { context, .. }
            | WeaverCliError::InvalidRange { context, .. }
            | WeaverCliError::MalformedEditJson { context, .. }
            | WeaverCliError::EditWireFrameTooLarge { context, .. }
            | WeaverCliError::BufferNotOpenedSave { context, .. } => context.as_deref(),
        }
    }

    pub fn fact_key(&self) -> Option<&FactKey> {
        match self {
            WeaverCliError::FactNotFound { key, .. } => Some(key),
            _ => None,
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            WeaverCliError::CoreUnavailable { .. } | WeaverCliError::FactNotFound { .. } => {
                exit_code::EXPECTED
            }
            WeaverCliError::ParseError { .. }
            | WeaverCliError::ProtocolError { .. }
            | WeaverCliError::BufferNotOpened { .. }
            | WeaverCliError::InvalidRange { .. }
            | WeaverCliError::MalformedEditJson { .. }
            | WeaverCliError::EditWireFrameTooLarge { .. }
            | WeaverCliError::BufferNotOpenedSave { .. } => exit_code::GENERAL,
        }
    }

    fn envelope(&self) -> ErrorEnvelope {
        ErrorEnvelope {
            error: ErrorBody {
                category: self.category(),
                code: self.code_str(),
                message: self.to_string(),
                context: self.context().map(str::to_string),
                fact_key: self.fact_key().map(FactKeyJson::from),
            },
        }
    }
}

/// Render an error to stdout in the requested format and return the
/// conventional exit code for the error class. Human mode prints the
/// miette diagnostic to stderr; JSON mode prints the envelope to
/// stdout so the shape is captured by `-o json | jq` pipelines.
pub fn render_error(err: &WeaverCliError, format: OutputFormat) -> miette::Result<()> {
    match format {
        OutputFormat::Human => {
            // Delegate to miette's fancy report handler via eprintln.
            eprintln!("{:?}", miette::Report::new(err.clone_err()));
            Ok(())
        }
        OutputFormat::Json => {
            let s = serde_json::to_string_pretty(&err.envelope()).into_diagnostic()?;
            println!("{s}");
            Ok(())
        }
    }
}

impl WeaverCliError {
    fn clone_err(&self) -> Self {
        // miette::Report::new_boxed requires an owned Diagnostic; our
        // variants are all Clone-compatible (FactKey is Clone).
        match self {
            WeaverCliError::CoreUnavailable { message, context } => {
                WeaverCliError::CoreUnavailable {
                    message: message.clone(),
                    context: context.clone(),
                }
            }
            WeaverCliError::FactNotFound { key, context } => WeaverCliError::FactNotFound {
                key: key.clone(),
                context: context.clone(),
            },
            WeaverCliError::ParseError { message, context } => WeaverCliError::ParseError {
                message: message.clone(),
                context: context.clone(),
            },
            WeaverCliError::ProtocolError { message, context } => WeaverCliError::ProtocolError {
                message: message.clone(),
                context: context.clone(),
            },
            WeaverCliError::BufferNotOpened {
                path,
                entity,
                context,
            } => WeaverCliError::BufferNotOpened {
                path: path.clone(),
                entity: *entity,
                context: context.clone(),
            },
            WeaverCliError::InvalidRange {
                input,
                detail,
                context,
            } => WeaverCliError::InvalidRange {
                input: input.clone(),
                detail: detail.clone(),
                context: context.clone(),
            },
            WeaverCliError::MalformedEditJson { detail, context } => {
                WeaverCliError::MalformedEditJson {
                    detail: detail.clone(),
                    context: context.clone(),
                }
            }
            WeaverCliError::EditWireFrameTooLarge {
                actual_bytes,
                max_bytes,
                context,
            } => WeaverCliError::EditWireFrameTooLarge {
                actual_bytes: *actual_bytes,
                max_bytes: *max_bytes,
                context: context.clone(),
            },
            WeaverCliError::BufferNotOpenedSave {
                entity_arg,
                entity,
                context,
            } => WeaverCliError::BufferNotOpenedSave {
                entity_arg: entity_arg.clone(),
                entity: *entity,
                context: context.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::entity_ref::EntityRef;

    #[test]
    fn unavailable_envelope_shape() {
        let err = WeaverCliError::CoreUnavailable {
            message: "core not reachable at /tmp/x.sock".into(),
            context: Some("weaver status".into()),
        };
        let s = serde_json::to_string(&err.envelope()).unwrap();
        assert!(s.contains("\"category\":\"core-unavailable\""));
        assert!(s.contains("\"code\":\"WEAVER-002\""));
        assert!(s.contains("\"fact_key\":null"));
        assert!(s.contains("\"context\":\"weaver status\""));
    }

    #[test]
    fn fact_not_found_envelope_populates_fact_key() {
        let err = WeaverCliError::FactNotFound {
            key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
            context: Some("weaver inspect 1:buffer/dirty".into()),
        };
        let s = serde_json::to_string(&err.envelope()).unwrap();
        assert!(s.contains("\"fact_key\":{\"entity\":1,\"attribute\":\"buffer/dirty\"}"));
        assert!(s.contains("\"category\":\"fact-not-found\""));
        assert!(s.contains("\"code\":\"WEAVER-201\""));
    }

    #[test]
    fn exit_codes_match_cli_surface_contract() {
        assert_eq!(
            WeaverCliError::CoreUnavailable {
                message: "x".into(),
                context: None
            }
            .exit_code(),
            exit_code::EXPECTED,
        );
        assert_eq!(
            WeaverCliError::FactNotFound {
                key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
                context: None,
            }
            .exit_code(),
            exit_code::EXPECTED,
        );
    }

    #[test]
    fn buffer_not_opened_envelope_shape() {
        let err = WeaverCliError::BufferNotOpened {
            path: "/tmp/foo.txt".into(),
            entity: 0xDEAD_BEEF,
            context: Some("weaver edit /tmp/foo.txt".into()),
        };
        let s = serde_json::to_string(&err.envelope()).unwrap();
        assert!(s.contains("\"category\":\"buffer-not-opened\""));
        assert!(s.contains("\"code\":\"WEAVER-EDIT-001\""));
        assert!(s.contains("/tmp/foo.txt"));
        assert_eq!(err.exit_code(), exit_code::GENERAL);
    }

    #[test]
    fn malformed_edit_json_envelope_shape() {
        let err = WeaverCliError::MalformedEditJson {
            detail: "expected `[` at line 1 column 1".into(),
            context: Some("weaver edit-json /tmp/foo.txt".into()),
        };
        let s = serde_json::to_string(&err.envelope()).unwrap();
        assert!(s.contains("\"category\":\"malformed-edit-json\""));
        assert!(s.contains("\"code\":\"WEAVER-EDIT-003\""));
        assert!(s.contains("expected `["));
        assert_eq!(err.exit_code(), exit_code::GENERAL);
    }

    #[test]
    fn edit_wire_frame_too_large_envelope_shape() {
        let err = WeaverCliError::EditWireFrameTooLarge {
            actual_bytes: 70_000,
            max_bytes: 65_536,
            context: Some("weaver edit-json".into()),
        };
        let s = serde_json::to_string(&err.envelope()).unwrap();
        assert!(s.contains("\"category\":\"edit-wire-frame-too-large\""));
        assert!(s.contains("\"code\":\"WEAVER-EDIT-004\""));
        assert!(s.contains("70000"));
        assert!(s.contains("65536"));
        assert_eq!(err.exit_code(), exit_code::GENERAL);
    }

    #[test]
    fn buffer_not_opened_save_envelope_shape() {
        // Slice 005: WEAVER-SAVE-001 mirrors WEAVER-EDIT-001's "buffer
        // not opened" situation on the save surface — same category,
        // distinct code so traces grep cleanly per surface.
        let err = WeaverCliError::BufferNotOpenedSave {
            entity_arg: "/tmp/foo.txt".into(),
            entity: 0xDEAD_BEEF,
            context: Some("weaver save /tmp/foo.txt".into()),
        };
        let s = serde_json::to_string(&err.envelope()).unwrap();
        assert!(s.contains("\"category\":\"buffer-not-opened\""));
        assert!(s.contains("\"code\":\"WEAVER-SAVE-001\""));
        assert!(s.contains("/tmp/foo.txt"));
        assert_eq!(err.exit_code(), exit_code::GENERAL);
    }

    #[test]
    fn invalid_range_envelope_shape() {
        let err = WeaverCliError::InvalidRange {
            input: "0:0".into(),
            detail: "expected <start-line>:<start-char>-<end-line>:<end-char>".into(),
            context: None,
        };
        let s = serde_json::to_string(&err.envelope()).unwrap();
        assert!(s.contains("\"category\":\"invalid-range\""));
        assert!(s.contains("\"code\":\"WEAVER-EDIT-002\""));
        assert!(s.contains("0:0"));
        assert_eq!(err.exit_code(), exit_code::GENERAL);
    }
}
