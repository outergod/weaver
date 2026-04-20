//! `weaver simulate-edit` and `weaver simulate-clean` — publish one
//! `buffer/edited` or `buffer/cleaned` event and print a submission ack.
//!
//! Per `specs/001-hello-fact/contracts/cli-surfaces.md`: these are
//! one-shot CLI surfaces; fact arrival is observed separately via the
//! TUI (subscription) or `weaver status`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use miette::{IntoDiagnostic, miette};
use serde::Serialize;
use tokio::runtime::Builder;

use crate::bus::client::Client;
use crate::cli::args::OutputFormat;
use crate::cli::config::Config;
use crate::provenance::{Provenance, SourceId};
use crate::types::entity_ref::EntityRef;
use crate::types::event::{Event, EventPayload};
use crate::types::ids::EventId;
use crate::types::message::BusMessage;

/// The event kinds the CLI can simulate in slice 001.
#[derive(Copy, Clone, Debug)]
pub enum SimulationKind {
    Edit,
    Clean,
}

impl SimulationKind {
    fn event_name(self) -> &'static str {
        match self {
            SimulationKind::Edit => "buffer/edited",
            SimulationKind::Clean => "buffer/cleaned",
        }
    }

    fn payload(self) -> EventPayload {
        match self {
            SimulationKind::Edit => EventPayload::BufferEdited,
            SimulationKind::Clean => EventPayload::BufferCleaned,
        }
    }
}

/// Wire-level submission ack — mirrors `contracts/cli-surfaces.md`.
#[derive(Debug, Serialize)]
struct SubmissionAck {
    event_id: u64,
    name: &'static str,
    target: u64,
    submitted_at_ns: u64,
}

/// Run a `simulate-edit` / `simulate-clean` subcommand end-to-end.
pub fn run(
    kind: SimulationKind,
    buffer_id: u64,
    output: OutputFormat,
    socket_override: Option<PathBuf>,
) -> miette::Result<()> {
    let cfg = Config::from_cli(socket_override);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;
    runtime.block_on(async move {
        let mut client = Client::connect(&cfg.socket_path, "cli")
            .await
            .map_err(|e| miette!("{e}"))?;

        let target = EntityRef::new(buffer_id);
        let submitted_at_ns = now_ns();
        let event = Event {
            id: EventId::new(submitted_at_ns),
            name: kind.event_name().into(),
            target: Some(target),
            payload: kind.payload(),
            provenance: Provenance::new(SourceId::External("cli".into()), submitted_at_ns, None)
                .map_err(|e| miette!("{e}"))?,
        };
        let event_id_u64 = event.id.as_u64();
        client
            .send(&BusMessage::Event(event))
            .await
            .map_err(|e| miette!("{e}"))?;

        let ack = SubmissionAck {
            event_id: event_id_u64,
            name: kind.event_name(),
            target: buffer_id,
            submitted_at_ns,
        };
        match output {
            OutputFormat::Human => print_human(&ack),
            OutputFormat::Json => print_json(&ack)?,
        }
        Ok::<(), miette::Report>(())
    })
}

fn print_human(ack: &SubmissionAck) {
    println!(
        "submitted {} for EntityRef({}) — event id {}",
        ack.name, ack.target, ack.event_id
    );
}

fn print_json(ack: &SubmissionAck) -> miette::Result<()> {
    let s = serde_json::to_string_pretty(ack).into_diagnostic()?;
    println!("{s}");
    Ok(())
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
