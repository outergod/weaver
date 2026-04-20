//! Bus messages — the typed enum of all wire messages on the bus.
//!
//! CBOR-encoded on the wire per `docs/02-architecture.md` §3.1 and
//! `specs/001-hello-fact/contracts/bus-messages.md`.

use crate::provenance::Provenance;
use crate::types::event::Event;
use crate::types::fact::{Fact, FactKey};
use crate::types::ids::{BehaviorId, EventId};
use serde::{Deserialize, Serialize};

/// Bus protocol version — slice 001 ships v0.1.0 as `0x01`.
///
/// Public surface per L2 P7. Increments follow the policy in
/// `specs/001-hello-fact/contracts/bus-messages.md` §Versioning.
pub const BUS_PROTOCOL_VERSION: u8 = 0x01;

/// Semver-style string representation of [`BUS_PROTOCOL_VERSION`].
/// Used in CLI output (e.g., `weaver --version`).
pub const BUS_PROTOCOL_VERSION_STR: &str = "0.1.0";

/// The top-level enum of bus messages.
///
/// Serde representation is external tagging with `kebab-case` variant
/// names — a message serializes as `{"hello": {...}}`,
/// `{"fact-assert": {...}}`, etc. Kebab-case wire vocabulary per L2
/// Additional Constraints (Amendment 5); external tagging handles
/// newtype variants wrapping non-struct types cleanly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BusMessage {
    Hello(HelloMsg),
    Event(Event),
    FactAssert(Fact),
    FactRetract {
        key: FactKey,
        provenance: Provenance,
    },
    Subscribe(SubscribePattern),
    SubscribeAck {
        sequence: u64,
    },
    InspectRequest {
        request_id: u64,
        fact: FactKey,
    },
    InspectResponse {
        request_id: u64,
        result: Result<InspectionDetail, InspectionError>,
    },
    Lifecycle(LifecycleSignal),
    Error(ErrorMsg),
    /// One-shot snapshot request used by `weaver status`. Client →
    /// core. No payload — the response carries the current lifecycle
    /// signal, process uptime, and the fact-space snapshot.
    StatusRequest,
    /// Response to [`BusMessage::StatusRequest`]. Core → client.
    StatusResponse {
        lifecycle: LifecycleSignal,
        uptime_ns: u64,
        facts: Vec<Fact>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloMsg {
    pub protocol_version: u8,
    pub client_kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubscribePattern {
    AllFacts,
    FamilyPrefix(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleSignal {
    Started,
    Ready,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub category: String,
    pub detail: String,
    pub context: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectionDetail {
    pub source_event: EventId,
    pub asserting_behavior: BehaviorId,
    pub asserted_at_ns: u64,
    pub trace_sequence: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InspectionError {
    FactNotFound,
    NoProvenance,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::SourceId;
    use crate::types::entity_ref::EntityRef;
    use crate::types::event::EventPayload;
    use crate::types::fact::FactValue;

    fn sample_fact() -> Fact {
        Fact {
            key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
            value: FactValue::Bool(true),
            provenance: Provenance::new(
                SourceId::Behavior(BehaviorId::new("core/dirty-tracking")),
                1000,
                Some(EventId::new(42)),
            )
            .unwrap(),
        }
    }

    fn sample_event() -> Event {
        Event {
            id: EventId::new(42),
            name: "buffer/edited".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferEdited,
            provenance: Provenance::new(SourceId::Tui, 999, None).unwrap(),
        }
    }

    fn all_variants() -> Vec<BusMessage> {
        vec![
            BusMessage::Hello(HelloMsg {
                protocol_version: BUS_PROTOCOL_VERSION,
                client_kind: "tui".into(),
            }),
            BusMessage::Event(sample_event()),
            BusMessage::FactAssert(sample_fact()),
            BusMessage::FactRetract {
                key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
                provenance: Provenance::new(SourceId::Core, 2000, None).unwrap(),
            },
            BusMessage::Subscribe(SubscribePattern::FamilyPrefix("buffer/".into())),
            BusMessage::Subscribe(SubscribePattern::AllFacts),
            BusMessage::SubscribeAck { sequence: 0 },
            BusMessage::InspectRequest {
                request_id: 7,
                fact: FactKey::new(EntityRef::new(1), "buffer/dirty"),
            },
            BusMessage::InspectResponse {
                request_id: 7,
                result: Ok(InspectionDetail {
                    source_event: EventId::new(42),
                    asserting_behavior: BehaviorId::new("core/dirty-tracking"),
                    asserted_at_ns: 1000,
                    trace_sequence: 17,
                }),
            },
            BusMessage::InspectResponse {
                request_id: 8,
                result: Err(InspectionError::FactNotFound),
            },
            BusMessage::Lifecycle(LifecycleSignal::Ready),
            BusMessage::Error(ErrorMsg {
                category: "protocol".into(),
                detail: "expected Hello".into(),
                context: None,
            }),
            BusMessage::StatusRequest,
            BusMessage::StatusResponse {
                lifecycle: LifecycleSignal::Ready,
                uptime_ns: 1_234_567_890,
                facts: vec![sample_fact()],
            },
        ]
    }

    #[test]
    fn ciborium_round_trip_every_variant() {
        for msg in all_variants() {
            let mut buf = Vec::new();
            ciborium::into_writer(&msg, &mut buf).unwrap();
            let back: BusMessage = ciborium::from_reader(buf.as_slice()).unwrap();
            assert_eq!(msg, back, "round-trip mismatch");
        }
    }

    #[test]
    fn json_round_trip_every_variant() {
        for msg in all_variants() {
            let s = serde_json::to_string(&msg).unwrap();
            let back: BusMessage = serde_json::from_str(&s).unwrap();
            assert_eq!(msg, back);
        }
    }
}
