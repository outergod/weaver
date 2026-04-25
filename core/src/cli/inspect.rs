//! `weaver inspect <entity-id>:<attribute>` — one-shot CLI wrapper over
//! the bus inspection request/response per FR-008.
//!
//! Shape of both output forms matches
//! `specs/001-hello-fact/contracts/cli-surfaces.md`.

use std::path::PathBuf;

use miette::{IntoDiagnostic, miette};
use serde::Serialize;
use thiserror::Error;
use tokio::runtime::Builder;

use crate::bus::client::{Client, ClientError};
use crate::cli::args::OutputFormat;
use crate::cli::config::Config;
use crate::types::entity_ref::EntityRef;
use crate::types::event::Event;
use crate::types::fact::FactKey;
use crate::types::message::{BusMessage, EventInspectionError, InspectionDetail, InspectionError};

/// Exit code for `weaver inspect` when the core is unreachable.
/// Distinct from `cli::errors::exit_code::EXPECTED` (=2, used for
/// `fact-not-found`) so scripts can distinguish the two failure modes
/// per `contracts/cli-surfaces.md`.
const EXIT_CORE_UNAVAILABLE: i32 = 3;

#[derive(Debug, Error)]
pub enum InspectCliError {
    #[error(
        "could not parse fact key `{input}` — expected `<entity-id>:<attribute>` (e.g., `1:buffer/dirty`)"
    )]
    InvalidKey { input: String },

    #[error("entity id `{raw}` is not a valid u64: {source}")]
    InvalidEntityId {
        raw: String,
        #[source]
        source: std::num::ParseIntError,
    },
}

/// JSON shape of a successful `weaver inspect` response. Fields mirror
/// `specs/002-git-watcher-actor/contracts/cli-surfaces.md`:
/// `asserting_behavior` for behavior-authored facts (slice 001);
/// `asserting_service` + `asserting_instance` for service-authored
/// facts (slice 002). At most one of those field groups is populated
/// per response.
#[derive(Debug, Serialize)]
struct FoundJson {
    fact: FactKeyJson,
    source_event: u64,
    /// Wire-level discriminator — one of `"behavior" | "service" |
    /// "core" | "tui" | "user" | "host" | "agent"`. Always present
    /// so consumers can parse the response without peeking at which
    /// identifier field happens to be populated (T064 review
    /// direction; mirrors `InspectionDetail::asserting_kind`).
    asserting_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    asserting_behavior: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asserting_service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asserting_instance: Option<String>,
    asserted_at_ns: u64,
    trace_sequence: u64,
}

#[derive(Debug, Serialize)]
struct NotFoundJson {
    fact: FactKeyJson,
    error: &'static str,
}

#[derive(Debug, Serialize)]
struct FactKeyJson {
    entity: u64,
    attribute: String,
}

/// Parse `<entity-id>:<attribute>` into a typed [`FactKey`].
pub fn parse_fact_key(input: &str) -> Result<FactKey, InspectCliError> {
    let (entity_raw, attribute) =
        input
            .split_once(':')
            .ok_or_else(|| InspectCliError::InvalidKey {
                input: input.to_string(),
            })?;
    if entity_raw.is_empty() || attribute.is_empty() {
        return Err(InspectCliError::InvalidKey {
            input: input.to_string(),
        });
    }
    let entity_id =
        entity_raw
            .parse::<u64>()
            .map_err(|source| InspectCliError::InvalidEntityId {
                raw: entity_raw.to_string(),
                source,
            })?;
    Ok(FactKey::new(EntityRef::new(entity_id), attribute))
}

/// Run `weaver inspect <fact-key> [--why]` end-to-end. Returns a
/// non-zero exit code (via the caller's `Result`) if the fact is not
/// found, per `cli-surfaces.md`.
///
/// Without `--why`: existing slice-001 behavior — one InspectRequest
/// → render `InspectionDetail` (or FactNotFound exit 2).
///
/// With `--why`: chain a second round-trip — take the returned
/// `InspectionDetail.source_event`, issue `EventInspectRequest`, and
/// render the walkback shape (per `specs/004-buffer-edit/contracts/
/// cli-surfaces.md §weaver inspect --why`). Exit 2 on either
/// FactNotFound or EventNotFound (both are "expected miss" per the
/// slice-001 inspect convention).
pub fn run(
    fact_key_str: &str,
    output: OutputFormat,
    why: bool,
    socket_override: Option<PathBuf>,
) -> miette::Result<()> {
    // Parse errors exit 1 (miette default) — they're caller input
    // errors, not a fact lookup outcome, so they don't fit either of
    // the subcommand-specific codes.
    let key = parse_fact_key(fact_key_str).map_err(|e| miette!("{e}"))?;
    let cfg = Config::from_cli(socket_override);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .into_diagnostic()?;
    runtime.block_on(async move {
        let mut client = match Client::connect(&cfg.socket_path, "cli").await {
            Ok(c) => c,
            Err(ClientError::Connect { path, source }) => {
                // Render the not-found shape with the core-unavailable
                // error so `--output=json` still produces parseable
                // output before exit 3.
                print_core_unavailable(&key, &path, &source, output)?;
                std::process::exit(EXIT_CORE_UNAVAILABLE);
            }
            Err(e) => return Err(miette!("{e}")),
        };

        let request_id = next_request_id();
        client
            .send(&BusMessage::InspectRequest {
                request_id,
                fact: key.clone(),
            })
            .await
            .map_err(|e| miette!("{e}"))?;

        let response = read_inspect_response(&mut client, request_id).await?;

        // Slice-004 --why: when set AND the fact resolved, chain a
        // second EventInspectRequest for the source-event walkback.
        if why {
            match &response {
                Ok(detail) => {
                    let event_id = detail.source_event;
                    let req_id = next_request_id();
                    client
                        .send(&BusMessage::EventInspectRequest {
                            request_id: req_id,
                            event_id,
                        })
                        .await
                        .map_err(|e| miette!("{e}"))?;
                    let event_resp = read_event_inspect_response(&mut client, req_id).await?;
                    render_walkback(&key, detail, &event_resp, output)?;
                    if event_resp.is_err() {
                        std::process::exit(crate::cli::errors::exit_code::EXPECTED);
                    }
                    return Ok(());
                }
                Err(_) => {
                    // FactNotFound under --why: render the slice-001
                    // not-found shape (no event to walk to) and exit 2.
                    render(&key, &response, output)?;
                    std::process::exit(crate::cli::errors::exit_code::EXPECTED);
                }
            }
        }

        render(&key, &response, output)?;
        // `FactNotFound` is a documented outcome, not a crash. The
        // contract says exit 2; route through `std::process::exit` so
        // scripts can distinguish it from (a) a generic crash (exit 1)
        // and (b) core-unavailable (exit 3 above).
        if response.is_err() {
            std::process::exit(crate::cli::errors::exit_code::EXPECTED);
        }
        Ok(())
    })
}

fn print_core_unavailable(
    key: &FactKey,
    path: &str,
    source: &std::io::Error,
    output: OutputFormat,
) -> miette::Result<()> {
    let message = format!("core not reachable at {path}: {source}");
    match output {
        OutputFormat::Human => {
            eprintln!("fact: ({}, {})", key.entity, key.attribute);
            eprintln!("  error: {message}");
        }
        OutputFormat::Json => {
            let envelope = crate::cli::errors::WeaverCliError::CoreUnavailable {
                message,
                context: Some(format!("weaver inspect {}:{}", key.entity, key.attribute)),
            };
            crate::cli::errors::render_error(&envelope, output)?;
        }
    }
    Ok(())
}

async fn read_inspect_response(
    client: &mut Client,
    expected_id: u64,
) -> miette::Result<Result<InspectionDetail, InspectionError>> {
    // The handler only sends us InspectResponse back — but FactAssert
    // / FactRetract could arrive from *other* subscribers if we had
    // subscribed. The one-shot CLI never subscribes, so the first
    // message after the request is always the response.
    loop {
        let msg = client.recv().await.map_err(|e| miette!("{e}"))?;
        match msg {
            BusMessage::InspectResponse { request_id, result } if request_id == expected_id => {
                return Ok(result);
            }
            // Defensive: ignore spurious frames (shouldn't happen on a
            // non-subscribed connection).
            _ => continue,
        }
    }
}

async fn read_event_inspect_response(
    client: &mut Client,
    expected_id: u64,
) -> miette::Result<Result<Event, EventInspectionError>> {
    loop {
        let msg = client.recv().await.map_err(|e| miette!("{e}"))?;
        match msg {
            BusMessage::EventInspectResponse { request_id, result }
                if request_id == expected_id =>
            {
                return Ok(result);
            }
            _ => continue,
        }
    }
}

fn render(
    key: &FactKey,
    response: &Result<InspectionDetail, InspectionError>,
    output: OutputFormat,
) -> miette::Result<()> {
    match (response, output) {
        (Ok(detail), OutputFormat::Human) => print_found_human(key, detail),
        (Ok(detail), OutputFormat::Json) => print_found_json(key, detail)?,
        (Err(e), OutputFormat::Human) => print_not_found_human(key, e),
        (Err(e), OutputFormat::Json) => print_not_found_json(key, e)?,
    }
    Ok(())
}

/// Slice-004 `--why` walkback rendering. Combines the existing
/// fact-inspect output with the source-event's `Event` envelope (or
/// an `event-not-found` marker if the trace lookup failed).
fn render_walkback(
    key: &FactKey,
    detail: &InspectionDetail,
    event_resp: &Result<Event, EventInspectionError>,
    output: OutputFormat,
) -> miette::Result<()> {
    match output {
        OutputFormat::Human => {
            print_found_human(key, detail);
            match event_resp {
                Ok(event) => print_event_human(event),
                Err(e) => println!("event: error: {}", event_inspection_error_label(e)),
            }
        }
        OutputFormat::Json => {
            print_walkback_json(key, detail, event_resp)?;
        }
    }
    Ok(())
}

fn print_event_human(event: &Event) {
    println!("event: {}", event.id);
    println!("  name:               {}", event.name);
    if let Some(t) = event.target {
        println!("  target:             {t}");
    } else {
        println!("  target:             —");
    }
    println!("  payload_type:       {}", event.payload.type_tag());
    println!(
        "  provenance.source:  {}",
        event.provenance.source.identifying_label()
    );
    println!("  timestamp_ns:       {}", event.provenance.timestamp_ns);
    if let Some(parent) = event.provenance.causal_parent {
        println!("  causal_parent:      {parent}");
    } else {
        println!("  causal_parent:      —");
    }
}

fn print_walkback_json(
    key: &FactKey,
    detail: &InspectionDetail,
    event_resp: &Result<Event, EventInspectionError>,
) -> miette::Result<()> {
    // Build the fact_inspection block by serialising the existing
    // FoundJson shape, so the inner field set stays in lockstep with
    // the no-`--why` rendering. A custom WalkbackJson with all field
    // names duplicated would silently drift over time.
    let fact_inspection = FoundJson {
        fact: FactKeyJson {
            entity: key.entity.as_u64(),
            attribute: key.attribute.clone(),
        },
        source_event: detail.source_event.as_u64(),
        asserting_kind: detail.asserting_kind.clone(),
        asserting_behavior: detail.asserting_behavior.as_ref().map(|b| b.to_string()),
        asserting_service: detail.asserting_service.clone(),
        asserting_instance: detail.asserting_instance.map(|u| u.to_string()),
        asserted_at_ns: detail.asserted_at_ns,
        trace_sequence: detail.trace_sequence,
    };
    let event_value = match event_resp {
        Ok(event) => {
            let payload_type = event.payload.type_tag();
            // Provenance is already Serialize; wrap to keep the wire
            // shape contract self-documenting (and to avoid leaking
            // private struct field names if Provenance ever evolves
            // without a wire bump).
            let provenance = serde_json::to_value(&event.provenance).into_diagnostic()?;
            serde_json::json!({
                "id": event.id.as_u64(),
                "name": event.name,
                "target": event.target.map(|e| e.as_u64()),
                "payload_type": payload_type,
                "provenance": provenance,
            })
        }
        Err(e) => serde_json::json!({
            "error": event_inspection_error_label(e),
        }),
    };
    let payload = serde_json::json!({
        "fact": serde_json::to_value(&fact_inspection.fact).into_diagnostic()?,
        "fact_inspection": serde_json::to_value(&fact_inspection).into_diagnostic()?,
        "event": event_value,
    });
    let s = serde_json::to_string_pretty(&payload).into_diagnostic()?;
    println!("{s}");
    Ok(())
}

fn event_inspection_error_label(e: &EventInspectionError) -> &'static str {
    match e {
        EventInspectionError::EventNotFound => "EventNotFound",
    }
}

fn print_found_human(key: &FactKey, d: &InspectionDetail) {
    println!("fact: ({}, {})", key.entity, key.attribute);
    println!("  source_event:       {}", d.source_event);
    println!("  asserting_kind:     {}", d.asserting_kind);
    if let Some(b) = &d.asserting_behavior {
        println!("  asserting_behavior: {b}");
    }
    if let Some(svc) = &d.asserting_service {
        println!("  asserting_service:  {svc}");
    }
    if let Some(inst) = &d.asserting_instance {
        println!("  asserting_instance: {inst}");
    }
    println!("  asserted_at_ns:     {}", d.asserted_at_ns);
    println!("  trace_sequence:     {}", d.trace_sequence);
}

fn print_found_json(key: &FactKey, d: &InspectionDetail) -> miette::Result<()> {
    let payload = FoundJson {
        fact: FactKeyJson {
            entity: key.entity.as_u64(),
            attribute: key.attribute.clone(),
        },
        source_event: d.source_event.as_u64(),
        asserting_kind: d.asserting_kind.clone(),
        asserting_behavior: d.asserting_behavior.as_ref().map(|b| b.to_string()),
        asserting_service: d.asserting_service.clone(),
        asserting_instance: d.asserting_instance.map(|u| u.to_string()),
        asserted_at_ns: d.asserted_at_ns,
        trace_sequence: d.trace_sequence,
    };
    let s = serde_json::to_string_pretty(&payload).into_diagnostic()?;
    println!("{s}");
    Ok(())
}

fn print_not_found_human(key: &FactKey, e: &InspectionError) {
    println!("fact: ({}, {})", key.entity, key.attribute);
    println!("  error: {}", inspection_error_label(e));
}

fn print_not_found_json(key: &FactKey, e: &InspectionError) -> miette::Result<()> {
    let payload = NotFoundJson {
        fact: FactKeyJson {
            entity: key.entity.as_u64(),
            attribute: key.attribute.clone(),
        },
        error: inspection_error_label(e),
    };
    let s = serde_json::to_string_pretty(&payload).into_diagnostic()?;
    println!("{s}");
    Ok(())
}

fn inspection_error_label(e: &InspectionError) -> &'static str {
    // Mirror the Rust variant names — the contract examples use
    // PascalCase here (`"FactNotFound"`), not kebab-case, because they
    // appear at the CLI surface rather than on the bus.
    match e {
        InspectionError::FactNotFound => "FactNotFound",
        InspectionError::NoProvenance => "NoProvenance",
    }
}

/// Monotonic (per-process) request id for correlating responses.
fn next_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_simple() {
        let k = parse_fact_key("1:buffer/dirty").unwrap();
        assert_eq!(k.entity.as_u64(), 1);
        assert_eq!(k.attribute, "buffer/dirty");
    }

    #[test]
    fn parse_key_rejects_missing_colon() {
        assert!(matches!(
            parse_fact_key("no-colon"),
            Err(InspectCliError::InvalidKey { .. })
        ));
    }

    #[test]
    fn parse_key_rejects_empty_fields() {
        assert!(matches!(
            parse_fact_key(":buffer/dirty"),
            Err(InspectCliError::InvalidKey { .. })
        ));
        assert!(matches!(
            parse_fact_key("1:"),
            Err(InspectCliError::InvalidKey { .. })
        ));
    }

    #[test]
    fn parse_key_rejects_non_numeric_entity() {
        assert!(matches!(
            parse_fact_key("abc:buffer/dirty"),
            Err(InspectCliError::InvalidEntityId { .. })
        ));
    }
}
