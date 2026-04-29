//! CLI output: `StatusResponse` shape + human / JSON formatters.
//!
//! Per L2 P5 (amended): the CLI surface mirrors the bus vocabulary. The
//! `StatusResponse` struct here mirrors what the bus returns
//! (`BusMessage::StatusResponse`), with the two additional shapes from
//! `contracts/cli-surfaces.md`:
//!
//! * `lifecycle == "unavailable"` + `error` field when the core is not
//!   reachable (no facts, no uptime).
//! * Standard shape when the core is reachable.

use serde::{Deserialize, Serialize};

use crate::cli::args::OutputFormat;
use crate::types::fact::Fact;
use crate::types::message::LifecycleSignal;

/// The top-level shape of `weaver status --output=json`. Designed to
/// round-trip through `serde_json` (tests in
/// `core/tests/cli/status_round_trip.rs` verify).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    /// `"ready"` or `"unavailable"` — mirrors
    /// [`LifecycleSignal`] plus the CLI-only `unavailable` state.
    pub lifecycle: String,
    /// Nanoseconds since the core started. `None` when the core is
    /// unreachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_ns: Option<u64>,
    /// Currently-asserted facts. Empty when the core is unreachable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<Fact>,
    /// Human-readable explanation for `lifecycle == "unavailable"`.
    /// `None` when the core is reachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl StatusResponse {
    /// Construct a reachable-core response from the bus payload.
    pub fn reachable(lifecycle: LifecycleSignal, uptime_ns: u64, facts: Vec<Fact>) -> Self {
        Self {
            lifecycle: lifecycle_label(lifecycle).to_string(),
            uptime_ns: Some(uptime_ns),
            facts,
            error: None,
        }
    }

    /// Construct the documented unavailable shape.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            lifecycle: "unavailable".to_string(),
            uptime_ns: None,
            facts: Vec::new(),
            error: Some(reason.into()),
        }
    }

    pub fn is_unavailable(&self) -> bool {
        self.lifecycle == "unavailable"
    }
}

fn lifecycle_label(lifecycle: LifecycleSignal) -> &'static str {
    match lifecycle {
        LifecycleSignal::Started => "started",
        LifecycleSignal::Ready => "ready",
        LifecycleSignal::Degraded => "degraded",
        LifecycleSignal::Unavailable => "unavailable",
        LifecycleSignal::Restarting => "restarting",
        LifecycleSignal::Stopped => "stopped",
    }
}

/// Unified format dispatcher. Writes the rendered status to stdout.
pub fn render_status(response: &StatusResponse, format: OutputFormat) -> miette::Result<()> {
    match format {
        OutputFormat::Human => {
            print_human(response);
            Ok(())
        }
        OutputFormat::Json => print_json(response),
    }
}

fn print_human(r: &StatusResponse) {
    println!("lifecycle: {}", r.lifecycle);
    if let Some(uptime) = r.uptime_ns {
        println!("uptime:    {}", format_uptime(uptime));
    }
    if let Some(err) = &r.error {
        println!("error:     {err}");
    }
    println!("facts ({}):", r.facts.len());
    if r.facts.is_empty() {
        println!("  (none)");
    } else {
        for fact in &r.facts {
            println!(
                "  {}({}) = {}",
                fact.key.attribute,
                fact.key.entity,
                format_value(&fact.value),
            );
            println!("    by {}", format_source(&fact.provenance.source));
            if let Some(parent) = fact.provenance.causal_parent {
                println!("    caused by {parent}");
            }
        }
    }
}

fn print_json(r: &StatusResponse) -> miette::Result<()> {
    use miette::IntoDiagnostic;
    let s = serde_json::to_string_pretty(r).into_diagnostic()?;
    println!("{s}");
    Ok(())
}

fn format_uptime(ns: u64) -> String {
    let secs = (ns as f64) / 1_000_000_000.0;
    if secs < 60.0 {
        format!("{secs:.3}s")
    } else if secs < 3600.0 {
        format!("{:.1}m", secs / 60.0)
    } else {
        format!("{:.2}h", secs / 3600.0)
    }
}

fn format_value(v: &crate::types::fact::FactValue) -> String {
    use crate::types::fact::FactValue;
    match v {
        FactValue::Bool(b) => b.to_string(),
        FactValue::String(s) => format!("{s:?}"),
        FactValue::Int(n) => n.to_string(),
        FactValue::U64(n) => n.to_string(),
        FactValue::Null => "null".into(),
    }
}

fn format_source(src: &crate::provenance::ActorIdentity) -> String {
    use crate::provenance::ActorIdentity;
    match src {
        ActorIdentity::Core => "core".into(),
        ActorIdentity::Behavior { id } => format!("behavior:{id}"),
        ActorIdentity::Tui => "tui".into(),
        ActorIdentity::Service {
            service_id,
            instance_id,
        } => format!("service:{service_id}:{instance_id}"),
        ActorIdentity::User => "user".into(),
        ActorIdentity::Host { host_id, .. } => format!("host:{host_id}"),
        ActorIdentity::Agent { agent_id, .. } => format!("agent:{agent_id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{ActorIdentity, Provenance};
    use crate::types::entity_ref::EntityRef;
    use crate::types::fact::{FactKey, FactValue};
    use crate::types::ids::{BehaviorId, EventId};

    fn sample_response() -> StatusResponse {
        StatusResponse::reachable(
            LifecycleSignal::Ready,
            12_345_678_900,
            vec![Fact {
                key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
                value: FactValue::Bool(true),
                provenance: Provenance::new(
                    ActorIdentity::behavior(BehaviorId::new("core/dirty-tracking")),
                    12_340_000_000,
                    Some(EventId::for_testing(42)),
                )
                .unwrap(),
            }],
        )
    }

    #[test]
    fn json_round_trip_ready() {
        let r = sample_response();
        let s = serde_json::to_string(&r).unwrap();
        let back: StatusResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn json_round_trip_unavailable() {
        let r = StatusResponse::unavailable("core not reachable at /tmp/x.sock");
        let s = serde_json::to_string(&r).unwrap();
        let back: StatusResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
        assert_eq!(r.lifecycle, "unavailable");
        assert!(r.is_unavailable());
    }

    #[test]
    fn ready_shape_omits_error_field() {
        let r = sample_response();
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("\"error\""));
        assert!(s.contains("\"lifecycle\":\"ready\""));
        assert!(s.contains("\"uptime_ns\""));
    }

    #[test]
    fn unavailable_shape_omits_uptime_and_facts_when_empty() {
        let r = StatusResponse::unavailable("boom");
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("\"uptime_ns\""));
        assert!(!s.contains("\"facts\""));
        assert!(s.contains("\"error\":\"boom\""));
    }
}
