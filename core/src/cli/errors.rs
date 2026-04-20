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
}

impl WeaverCliError {
    /// One-line category string surfaced in both human and JSON modes.
    pub fn category(&self) -> &'static str {
        match self {
            WeaverCliError::CoreUnavailable { .. } => "core-unavailable",
            WeaverCliError::FactNotFound { .. } => "fact-not-found",
            WeaverCliError::ParseError { .. } => "parse-error",
            WeaverCliError::ProtocolError { .. } => "protocol-error",
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
        }
    }

    pub fn context(&self) -> Option<&str> {
        match self {
            WeaverCliError::CoreUnavailable { context, .. }
            | WeaverCliError::FactNotFound { context, .. }
            | WeaverCliError::ParseError { context, .. }
            | WeaverCliError::ProtocolError { context, .. } => context.as_deref(),
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
            WeaverCliError::ParseError { .. } | WeaverCliError::ProtocolError { .. } => {
                exit_code::GENERAL
            }
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
}
