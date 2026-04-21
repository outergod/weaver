//! `weaver status` — one-shot snapshot of core lifecycle + currently
//! asserted facts via the bus's `StatusRequest`/`StatusResponse`
//! message pair.
//!
//! Output shape per
//! `specs/001-hello-fact/contracts/cli-surfaces.md`.

use std::path::PathBuf;

use miette::IntoDiagnostic;
use tokio::runtime::Builder;

use crate::bus::client::{Client, ClientError};
use crate::cli::args::OutputFormat;
use crate::cli::config::Config;
use crate::cli::errors::{WeaverCliError, render_error};
use crate::cli::output::{StatusResponse, render_status};
use crate::types::message::BusMessage;

/// Run `weaver status` end-to-end. Returns a miette Result for the
/// caller to propagate; the exit-code convention is surfaced through
/// `run_with_exit` if the caller wants explicit control (see
/// `run_exit` below).
pub fn run(output: OutputFormat, socket_override: Option<PathBuf>) -> miette::Result<()> {
    let cfg = Config::from_cli(socket_override);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;
    runtime.block_on(async move {
        match fetch_status(&cfg.socket_path).await {
            Ok(response) => {
                render_status(&response, output)?;
                Ok(())
            }
            Err(err) => {
                // On unavailable, render the documented
                // `StatusResponse::unavailable` shape to stdout (per
                // the contract example), then exit 2 per
                // `contracts/cli-surfaces.md`.
                let response = StatusResponse::unavailable(err.to_string());
                render_status(&response, output)?;
                std::process::exit(err.exit_code());
            }
        }
    })
}

async fn fetch_status(socket: &std::path::Path) -> Result<StatusResponse, WeaverCliError> {
    let mut client = Client::connect(socket, "cli").await.map_err(|e| match e {
        ClientError::Connect { path, source } => WeaverCliError::CoreUnavailable {
            message: format!("core not reachable at {path}: {source}"),
            context: Some("weaver status".into()),
        },
        other => WeaverCliError::ProtocolError {
            message: other.to_string(),
            context: Some("weaver status — handshake".into()),
        },
    })?;

    client
        .send(&BusMessage::StatusRequest)
        .await
        .map_err(|e| WeaverCliError::ProtocolError {
            message: e.to_string(),
            context: Some("weaver status — send".into()),
        })?;

    match client.recv().await {
        Ok(BusMessage::StatusResponse {
            lifecycle,
            uptime_ns,
            facts,
        }) => Ok(StatusResponse::reachable(lifecycle, uptime_ns, facts)),
        Ok(other) => Err(WeaverCliError::ProtocolError {
            message: format!("unexpected response: {other:?}"),
            context: Some("weaver status".into()),
        }),
        Err(e) => Err(WeaverCliError::ProtocolError {
            message: e.to_string(),
            context: Some("weaver status — recv".into()),
        }),
    }
}

/// For callers that want the structured error envelope instead of the
/// contract-shaped `StatusResponse` (e.g., wrapping inside a larger
/// pipeline). Currently unused by the slice-001 surface but exported
/// for symmetry.
#[allow(dead_code)]
pub fn render_cli_error(err: &WeaverCliError, format: OutputFormat) -> miette::Result<()> {
    render_error(err, format)
}
