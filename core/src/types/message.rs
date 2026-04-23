//! Bus messages — the typed enum of all wire messages on the bus.
//!
//! CBOR-encoded on the wire per `docs/02-architecture.md` §3.1 and
//! `specs/001-hello-fact/contracts/bus-messages.md`.

use crate::provenance::Provenance;
use crate::types::event::Event;
use crate::types::fact::{Fact, FactKey};
use crate::types::ids::{BehaviorId, EventId};
use serde::{Deserialize, Serialize};

/// Bus protocol version.
///
/// - `0x01` / `0.1.0` — slice 001 shipped this with opaque
///   `SourceId::External(String)` in provenance.
/// - `0x02` / `0.2.0` — slice 002. Breaking wire change:
///   provenance carries structured [`crate::provenance::ActorIdentity`]
///   (new CBOR tag 1002), and [`LifecycleSignal`] gains
///   `Degraded` / `Unavailable` / `Restarting` variants.
/// - `0x03` / `0.3.0` — **current**, slice 003. Breaking wire change:
///   [`EventPayload`] drops `BufferEdited` / `BufferCleaned` in favor
///   of `BufferOpen { path }`; `FactValue` gains a `U64` variant. See
///   `specs/003-buffer-service/contracts/bus-messages.md`.
///
/// Public surface per L2 P7. Increments follow the policy in
/// `specs/003-buffer-service/contracts/bus-messages.md` §Versioning.
pub const BUS_PROTOCOL_VERSION: u8 = 0x03;

/// Semver-style string representation of [`BUS_PROTOCOL_VERSION`].
/// Used in CLI output (e.g., `weaver --version`).
pub const BUS_PROTOCOL_VERSION_STR: &str = "0.3.0";

/// The top-level enum of bus messages.
///
/// Wire shape (adjacent tagging with `"type"` discriminator and
/// `"payload"` content field, kebab-case variant names per L2
/// Amendment 5): `Hello(msg)` →
/// `{"type":"hello","payload":{...}}`; `StatusRequest` →
/// `{"type":"status-request"}` (unit variants omit `payload`);
/// `FactRetract { key, provenance }` →
/// `{"type":"fact-retract","payload":{"key":...,"provenance":...}}`.
/// Consumers always dispatch on `.type` regardless of variant shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]
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

/// Wire shape: adjacent tagging (`"type"` + `"pattern"`), kebab-case
/// variants. `AllFacts` → `{"type":"all-facts"}`; `FamilyPrefix(s)` →
/// `{"type":"family-prefix","pattern":"buffer/"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "pattern", rename_all = "kebab-case")]
pub enum SubscribePattern {
    AllFacts,
    FamilyPrefix(String),
}

impl SubscribePattern {
    /// Return `true` when a fact-key belongs to this subscription.
    ///
    /// `AllFacts` matches everything; `FamilyPrefix("buffer/")`
    /// matches any attribute whose string representation starts with
    /// `"buffer/"`. Shared between the listener (for snapshot-on-
    /// subscribe) and the fact store's internal matcher.
    pub fn matches(&self, key: &FactKey) -> bool {
        match self {
            SubscribePattern::AllFacts => true,
            SubscribePattern::FamilyPrefix(prefix) => key.attribute.starts_with(prefix.as_str()),
        }
    }
}

/// Service-lifecycle signal carried over the bus.
///
/// Slice 001 shipped `Started` / `Ready` / `Stopped` for the core's
/// own lifecycle. Slice 002 extends the enum with `Degraded` /
/// `Unavailable` / `Restarting` per `docs/05-protocols.md §5`; these
/// are emitted by services that can degrade without exiting (e.g.,
/// `weaver-git-watcher` losing observation of its repository
/// transiently).
///
/// The core continues to emit only `Started` / `Ready` / `Stopped`
/// this slice; the richer transitions are service-kind-specific.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecycleSignal {
    Started,
    Ready,
    Degraded,
    Unavailable,
    Restarting,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub category: String,
    pub detail: String,
    pub context: Option<String>,
}

/// Provenance detail returned by an `InspectRequest`.
///
/// Slice 002 extends the shape so service-authored facts render
/// alongside behavior-authored ones. The invariant is: exactly one of
/// the `asserting_*` field groups is populated per response.
///
/// - **Behavior-authored** (slice 001 shape): `asserting_behavior` is
///   `Some(id)`; `asserting_service` / `asserting_instance` are
///   `None`. Used when a registered in-core behavior fires and asserts
///   the fact through the dispatcher.
/// - **Service-authored** (slice 002, e.g. `weaver-git-watcher`):
///   `asserting_service` is `Some("git-watcher")` and
///   `asserting_instance` is `Some(<uuid-v4>)`; `asserting_behavior`
///   is `None`. Used when the fact arrives via a bare `FactAssert`
///   from an external service on the bus.
/// - **Other actor kinds** (`Core` / `Tui` / `User` / `Host` /
///   `Agent`): all three `asserting_*` fields are `None`, indicating
///   the fact is attributable to an actor whose identity doesn't
///   naturally reduce to "behavior" or "service" semantics. Rendering
///   falls back on `source_event` + the trace entry.
///
/// JSON wire shape:
///
/// - `asserting_kind` is **always present** on serialization —
///   `"behavior" | "service" | "core" | "tui" | "user" | "host" |
///   "agent"` — matching [`crate::provenance::ActorIdentity::kind_label`].
///   It is the wire-level discriminator that lets consumers parse
///   the response without peeking at which `asserting_*` identifier
///   field happens to be populated (T064 / T067 review direction).
///
/// - The identifier fields (`asserting_behavior`, `asserting_service`,
///   `asserting_instance`) are `#[serde(skip_serializing_if =
///   "Option::is_none")]` so each shape renders flat. Only the slice's
///   emitted kinds get identifier fields this slice:
///   - `"behavior"` → `asserting_behavior`
///   - `"service"` → `asserting_service`, `asserting_instance`
///   - `"core" | "tui"` → no identifier fields (the kind is the identity)
///   - reserved (`"user" | "host" | "agent"`) — `asserting_kind` only;
///     richer payload defers to the slice that actually emits them.
///
/// **Backward compatibility (F30 review fix)**: the slice added
/// `asserting_kind` as an additive, MINOR-grade field per
/// cli-surfaces.md §wire compatibility. Making it required on
/// deserialization would, however, break mixed-version deployments —
/// a new client could not decode an `InspectResponse` from a
/// pre-upgrade core that still omits the field. `InspectionDetail`
/// therefore deserializes through [`InspectionDetailRepr`] (via
/// `#[serde(from = ...)]`): when `asserting_kind` is absent we infer
/// it from the identifier fields (`behavior` if
/// `asserting_behavior` is populated, `service` if `asserting_service`
/// is populated, else `core` as a lossy reconstruction of the
/// pre-slice `opaque()` case). Serialization is unaffected — new
/// producers always emit the field.
///
/// See `specs/002-git-watcher-actor/contracts/cli-surfaces.md`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "InspectionDetailRepr")]
pub struct InspectionDetail {
    pub source_event: EventId,
    pub asserting_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asserting_behavior: Option<BehaviorId>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "asserting_service")]
    pub asserting_service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "asserting_instance")]
    pub asserting_instance: Option<uuid::Uuid>,
    pub asserted_at_ns: u64,
    pub trace_sequence: u64,
}

/// Wire-compat deserialization shape for [`InspectionDetail`].
/// `asserting_kind` is optional here so pre-upgrade-core responses
/// (which never emit the field) still decode; the `From` impl below
/// fills in a best-effort kind when absent.
#[derive(Deserialize)]
struct InspectionDetailRepr {
    source_event: EventId,
    #[serde(default)]
    asserting_kind: Option<String>,
    asserting_behavior: Option<BehaviorId>,
    asserting_service: Option<String>,
    asserting_instance: Option<uuid::Uuid>,
    asserted_at_ns: u64,
    trace_sequence: u64,
}

impl From<InspectionDetailRepr> for InspectionDetail {
    fn from(r: InspectionDetailRepr) -> Self {
        let asserting_kind = r.asserting_kind.unwrap_or_else(|| {
            // Inference rule for pre-slice responses — mirrors the
            // pre-T064 three-shape partitioning: behavior /
            // service / opaque. "core" is the lossy default for
            // the third group (Core / Tui / reserved variants
            // were indistinguishable on the old wire anyway).
            if r.asserting_behavior.is_some() {
                "behavior".into()
            } else if r.asserting_service.is_some() {
                "service".into()
            } else {
                "core".into()
            }
        });
        Self {
            source_event: r.source_event,
            asserting_kind,
            asserting_behavior: r.asserting_behavior,
            asserting_service: r.asserting_service,
            asserting_instance: r.asserting_instance,
            asserted_at_ns: r.asserted_at_ns,
            trace_sequence: r.trace_sequence,
        }
    }
}

impl InspectionDetail {
    /// Build an `InspectionDetail` for a behavior-authored fact.
    /// Slice-001 shape extended with `asserting_kind = "behavior"`.
    pub fn behavior(
        source_event: EventId,
        asserting_behavior: BehaviorId,
        asserted_at_ns: u64,
        trace_sequence: u64,
    ) -> Self {
        Self {
            source_event,
            asserting_kind: "behavior".into(),
            asserting_behavior: Some(asserting_behavior),
            asserting_service: None,
            asserting_instance: None,
            asserted_at_ns,
            trace_sequence,
        }
    }

    /// Build an `InspectionDetail` for a service-authored fact.
    /// Slice-002 shape extended with `asserting_kind = "service"`.
    pub fn service(
        source_event: EventId,
        service_id: String,
        instance_id: uuid::Uuid,
        asserted_at_ns: u64,
        trace_sequence: u64,
    ) -> Self {
        Self {
            source_event,
            asserting_kind: "service".into(),
            asserting_behavior: None,
            asserting_service: Some(service_id),
            asserting_instance: Some(instance_id),
            asserted_at_ns,
            trace_sequence,
        }
    }

    /// Build an `InspectionDetail` for a fact whose actor kind does
    /// not carry a separately-rendered identifier this slice (Core,
    /// Tui, or the reserved User/Host/Agent variants). Callers pass
    /// the kind label string via
    /// [`crate::provenance::ActorIdentity::kind_label`] so the
    /// wire-level discriminator is always meaningful (T064 review
    /// fix: previously this constructor produced a purely opaque
    /// response with no way for consumers to distinguish Core from
    /// Tui).
    pub fn kind_only(
        kind: &'static str,
        source_event: EventId,
        asserted_at_ns: u64,
        trace_sequence: u64,
    ) -> Self {
        Self {
            source_event,
            asserting_kind: kind.into(),
            asserting_behavior: None,
            asserting_service: None,
            asserting_instance: None,
            asserted_at_ns,
            trace_sequence,
        }
    }
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
    use crate::provenance::ActorIdentity;
    use crate::types::entity_ref::EntityRef;
    use crate::types::event::EventPayload;
    use crate::types::fact::FactValue;

    fn sample_fact() -> Fact {
        Fact {
            key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
            value: FactValue::Bool(true),
            provenance: Provenance::new(
                ActorIdentity::behavior(BehaviorId::new("core/dirty-tracking")),
                1000,
                Some(EventId::new(42)),
            )
            .unwrap(),
        }
    }

    fn sample_event() -> Event {
        Event {
            id: EventId::new(42),
            name: "buffer/open".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(ActorIdentity::Tui, 999, None).unwrap(),
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
                provenance: Provenance::new(ActorIdentity::Core, 2000, None).unwrap(),
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
                result: Ok(InspectionDetail::behavior(
                    EventId::new(42),
                    BehaviorId::new("core/dirty-tracking"),
                    1000,
                    17,
                )),
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

    /// F30 regression: a pre-slice `InspectResponse` (no
    /// `asserting_kind` field) must still decode cleanly.
    /// Behavior-authored shape infers `"behavior"` from the
    /// presence of `asserting_behavior`.
    #[test]
    fn inspection_detail_decodes_legacy_behavior_shape() {
        let legacy = r#"{
            "source_event": 42,
            "asserting_behavior": "core/dirty-tracking",
            "asserted_at_ns": 1000,
            "trace_sequence": 7
        }"#;
        let d: InspectionDetail = serde_json::from_str(legacy).expect("decode");
        assert_eq!(d.asserting_kind, "behavior");
        assert_eq!(
            d.asserting_behavior,
            Some(BehaviorId::new("core/dirty-tracking"))
        );
        assert_eq!(d.source_event, EventId::new(42));
    }

    /// F30 regression: service-authored legacy shape infers
    /// `"service"` from the presence of `asserting_service`.
    #[test]
    fn inspection_detail_decodes_legacy_service_shape() {
        let legacy = r#"{
            "source_event": 117,
            "asserting_service": "git-watcher",
            "asserting_instance": "2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90",
            "asserted_at_ns": 1000,
            "trace_sequence": 7
        }"#;
        let d: InspectionDetail = serde_json::from_str(legacy).expect("decode");
        assert_eq!(d.asserting_kind, "service");
        assert_eq!(d.asserting_service.as_deref(), Some("git-watcher"));
        assert!(d.asserting_instance.is_some());
    }

    /// F30 regression: the pre-slice opaque shape (all identifier
    /// fields absent) decodes with the lossy `"core"` fallback —
    /// pre-T064 the wire couldn't distinguish Core / Tui / reserved
    /// anyway, so inference can't recover the exact variant.
    #[test]
    fn inspection_detail_decodes_legacy_opaque_shape() {
        let legacy = r#"{
            "source_event": 9,
            "asserted_at_ns": 1000,
            "trace_sequence": 7
        }"#;
        let d: InspectionDetail = serde_json::from_str(legacy).expect("decode");
        assert_eq!(d.asserting_kind, "core");
        assert!(d.asserting_behavior.is_none());
        assert!(d.asserting_service.is_none());
    }
}
