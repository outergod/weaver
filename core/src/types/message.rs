//! Bus messages — the typed enum of all wire messages on the bus.
//!
//! CBOR-encoded on the wire per `docs/02-architecture.md` §3.1 and
//! `specs/001-hello-fact/contracts/bus-messages.md`.

use crate::provenance::Provenance;
use crate::types::event::Event;
use crate::types::fact::{Fact, FactKey, FactValue};
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
/// - `0x03` / `0.3.0` — slice 003. Breaking wire change:
///   [`EventPayload`] drops `BufferEdited` / `BufferCleaned` in favor
///   of `BufferOpen { path }`; `FactValue` gains a `U64` variant. See
///   `specs/003-buffer-service/contracts/bus-messages.md`.
/// - `0x04` / `0.4.0` — slice 004. Breaking wire change:
///   [`crate::types::event::EventPayload`] gains a `BufferEdit { entity,
///   version, edits }` variant carrying the new
///   [`crate::types::edit::TextEdit`] / [`crate::types::edit::Range`] /
///   [`crate::types::edit::Position`] struct types. See
///   `specs/004-buffer-edit/contracts/bus-messages.md`.
/// - `0x05` / `0.5.0` — **current**, slice 005. Breaking wire change:
///   adds the `buffer-save` event variant; producers serialise an
///   ID-stripped event envelope, with the bus listener allocating a
///   fresh [`crate::types::ids::EventId`] on accept (closes the
///   wall-clock-ns collision class — see `docs/07-open-questions.md`
///   §28). See `specs/005-buffer-save/contracts/bus-messages.md`.
///
/// Public surface per L2 P7. Increments follow the policy in
/// `specs/003-buffer-service/contracts/bus-messages.md` §Versioning.
pub const BUS_PROTOCOL_VERSION: u8 = 0x05;

/// Semver-style string representation of [`BUS_PROTOCOL_VERSION`].
/// Used in CLI output (e.g., `weaver --version`).
pub const BUS_PROTOCOL_VERSION_STR: &str = "0.5.0";

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
    /// Subscribe to events matching an [`EventSubscribePattern`]
    /// (slice 004 — see `specs/004-buffer-edit/research.md §13`).
    /// Lossy-class delivery: the core forwards each
    /// [`BusMessage::Event`] whose payload type-tag matches the
    /// pattern. Replied with [`BusMessage::SubscribeAck`] using
    /// `sequence: 0` (events have no per-publisher sequence).
    /// A second `SubscribeEvents` on the same connection replaces the
    /// prior pattern (last-wins; mirrors the fact-subscription
    /// convention).
    SubscribeEvents(EventSubscribePattern),
    InspectRequest {
        request_id: u64,
        fact: FactKey,
    },
    InspectResponse {
        request_id: u64,
        result: Result<InspectionDetail, InspectionError>,
    },
    /// Look up an event in the core's trace by id (slice 004 — see
    /// `specs/004-buffer-edit/research.md §14`). Powers
    /// `weaver inspect --why`'s chain walk: a fact's
    /// `InspectionDetail.source_event` resolves to a `TraceSequence`
    /// via `TraceStore::find_event`, which the listener fetches and
    /// returns as the full `Event` envelope.
    EventInspectRequest {
        request_id: u64,
        event_id: EventId,
    },
    EventInspectResponse {
        request_id: u64,
        result: Result<Event, EventInspectionError>,
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

/// Event-subscription pattern (slice 004). Matches against an
/// [`crate::types::event::EventPayload`]'s adjacent-tag discriminator.
///
/// Wire shape: adjacent tagging (`"type"` + `"pattern"`), kebab-case
/// variant names (symmetric with [`SubscribePattern`]). For example,
/// `PayloadType("buffer-edit".into())` →
/// `{"type":"payload-type","pattern":"buffer-edit"}`.
///
/// **Why a separate enum from [`SubscribePattern`]**: facts and events
/// have structurally different delivery classes (authoritative vs
/// lossy), reconnect semantics (snapshot vs no-replay), and idempotence
/// semantics. Folding them into one pattern would conflate concerns
/// the listener already handles distinctly. See
/// `specs/004-buffer-edit/research.md §13`.
///
/// Future variants (out of slice-004 scope; documented for the design
/// ceiling): `TargetEntity(EntityRef)` for per-entity filtering;
/// `EventNamePrefix(String)` for `Event.name`-based filtering. Deferred
/// until a concrete user case appears.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "pattern", rename_all = "kebab-case")]
pub enum EventSubscribePattern {
    /// Match events whose [`EventPayload`]'s
    /// [`crate::types::event::EventPayload::type_tag`] equals this string
    /// (e.g., `"buffer-edit"`, `"buffer-open"`). Matches any target
    /// entity; subscribers that only care about specific targets MUST
    /// filter at receipt time.
    PayloadType(String),
    /// Match events whose payload type tag is in the provided list —
    /// equivalent to a logical OR of multiple `PayloadType`
    /// subscriptions on a single connection. Required because the
    /// listener implements last-wins per-connection: two consecutive
    /// `SubscribeEvents` calls would drop the first subscription.
    /// Slice 005 introduces this variant so `weaver-buffers` can
    /// receive both `buffer-edit` and `buffer-save` events through
    /// one subscription handle.
    PayloadTypes(Vec<String>),
}

impl EventSubscribePattern {
    /// Return `true` when the event matches this subscription.
    pub fn matches(&self, event: &crate::types::event::Event) -> bool {
        let tag = event.payload.type_tag();
        match self {
            Self::PayloadType(t) => t.as_str() == tag,
            Self::PayloadTypes(ts) => ts.iter().any(|t| t.as_str() == tag),
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
/// **Backward compatibility (F30 review fix)**: the slice-002 update
/// added `asserting_kind` as an additive, MINOR-grade field per
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
/// **Slice-004 mid-flight extension**: the `value` field is added as
/// REQUIRED on the wire — the 0x04 protocol-mismatch handshake
/// already rejects mixed-version clients, so a separate compat shim
/// would be dead code. Slice-004's `weaver edit` emitter consumes
/// `value` to extract the current `buffer/version` for the
/// `BufferEdit` envelope. See `specs/004-buffer-edit/research.md §2`.
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
    /// The fact's current value at inspection time (slice-004
    /// addition). Slices 001-003 returned only provenance via this
    /// struct; slice 004's `weaver edit` emitter needs the value to
    /// construct `BufferEdit { entity, version, .. }` from the
    /// current `buffer/version` fact.
    pub value: FactValue,
}

/// Wire-compat deserialization shape for [`InspectionDetail`].
/// `asserting_kind` is optional here so pre-upgrade-core responses
/// (which never emit the field) still decode; the `From` impl below
/// fills in a best-effort kind when absent. `value` is REQUIRED on
/// the wire post-slice-004 (the 0x04 handshake rejects pre-slice-004
/// clients, so there's no version-mixed deployment to compat-shim).
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
    value: FactValue,
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
            value: r.value,
        }
    }
}

impl InspectionDetail {
    /// Build an `InspectionDetail` for a behavior-authored fact.
    /// Slice-001 shape extended with `asserting_kind = "behavior"`
    /// (slice 002) and `value` (slice 004).
    pub fn behavior(
        source_event: EventId,
        asserting_behavior: BehaviorId,
        asserted_at_ns: u64,
        trace_sequence: u64,
        value: FactValue,
    ) -> Self {
        Self {
            source_event,
            asserting_kind: "behavior".into(),
            asserting_behavior: Some(asserting_behavior),
            asserting_service: None,
            asserting_instance: None,
            asserted_at_ns,
            trace_sequence,
            value,
        }
    }

    /// Build an `InspectionDetail` for a service-authored fact.
    /// Slice-002 shape extended with `asserting_kind = "service"`
    /// and (slice 004) `value`.
    pub fn service(
        source_event: EventId,
        service_id: String,
        instance_id: uuid::Uuid,
        asserted_at_ns: u64,
        trace_sequence: u64,
        value: FactValue,
    ) -> Self {
        Self {
            source_event,
            asserting_kind: "service".into(),
            asserting_behavior: None,
            asserting_service: Some(service_id),
            asserting_instance: Some(instance_id),
            asserted_at_ns,
            trace_sequence,
            value,
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
        value: FactValue,
    ) -> Self {
        Self {
            source_event,
            asserting_kind: kind.into(),
            asserting_behavior: None,
            asserting_service: None,
            asserting_instance: None,
            asserted_at_ns,
            trace_sequence,
            value,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InspectionError {
    FactNotFound,
    NoProvenance,
}

/// Failure modes for [`BusMessage::EventInspectRequest`]. Slice 004
/// ships only `EventNotFound`; the trace's [`crate::trace::store::TraceStore`]
/// either has the event or doesn't. Other failure modes (decode errors,
/// protocol violations) are caught at the listener boundary, not as
/// `EventInspectResponse` errors.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventInspectionError {
    EventNotFound,
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
                Some(EventId::for_testing(42)),
            )
            .unwrap(),
        }
    }

    fn sample_event() -> Event {
        Event {
            id: EventId::for_testing(42),
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
                    EventId::for_testing(42),
                    BehaviorId::new("core/dirty-tracking"),
                    1000,
                    17,
                    FactValue::Bool(true),
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
            BusMessage::EventInspectRequest {
                request_id: 9,
                event_id: EventId::for_testing(0xCAFE),
            },
            BusMessage::EventInspectResponse {
                request_id: 9,
                result: Ok(sample_event()),
            },
            BusMessage::EventInspectResponse {
                request_id: 10,
                result: Err(EventInspectionError::EventNotFound),
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

    /// F30 regression: a pre-slice-002 `InspectResponse` (no
    /// `asserting_kind` field) must still decode cleanly under the
    /// repr's inference rule. Behavior-authored shape infers
    /// `"behavior"` from the presence of `asserting_behavior`.
    ///
    /// Slice-004 update: `value` is a REQUIRED field on the wire post-
    /// 0x04 (the protocol-mismatch handshake rejects mixed-version
    /// clients, so a missing-value compat shim would be unreachable).
    /// The fixture below carries `value` to exercise the post-0x04
    /// shape; the asserting_kind inference path being tested is
    /// orthogonal to the value field's presence.
    #[test]
    fn inspection_detail_decodes_legacy_behavior_shape() {
        // Slice 005 §28(a) re-derivation: `source_event` is now a
        // UUIDv8 hex string on the wire (was u64 in slice 004). The
        // "legacy" framing here refers to the pre-T064 absence of
        // `asserting_kind` discrimination, NOT to the slice-004
        // EventId u64 wire shape — which the protocol-mismatch
        // handshake rejects under bump 0x04 → 0x05.
        let legacy = r#"{
            "source_event": "00000000-0000-0000-0000-00000000002a",
            "asserting_behavior": "core/dirty-tracking",
            "asserted_at_ns": 1000,
            "trace_sequence": 7,
            "value": {"type": "bool", "value": true}
        }"#;
        let d: InspectionDetail = serde_json::from_str(legacy).expect("decode");
        assert_eq!(d.asserting_kind, "behavior");
        assert_eq!(
            d.asserting_behavior,
            Some(BehaviorId::new("core/dirty-tracking"))
        );
        assert_eq!(d.source_event, EventId::for_testing(42));
        assert_eq!(d.value, FactValue::Bool(true));
    }

    /// F30 regression: service-authored legacy shape infers
    /// `"service"` from the presence of `asserting_service`. Carries
    /// `value` per the slice-004 wire requirement (see the behavior-
    /// shape test above for context).
    #[test]
    fn inspection_detail_decodes_legacy_service_shape() {
        let legacy = r#"{
            "source_event": "00000000-0000-0000-0000-000000000075",
            "asserting_service": "git-watcher",
            "asserting_instance": "2e1a4f8b-4d13-4b0e-b4e3-6a6b00b35c90",
            "asserted_at_ns": 1000,
            "trace_sequence": 7,
            "value": {"type": "string", "value": "ready"}
        }"#;
        let d: InspectionDetail = serde_json::from_str(legacy).expect("decode");
        assert_eq!(d.asserting_kind, "service");
        assert_eq!(d.asserting_service.as_deref(), Some("git-watcher"));
        assert!(d.asserting_instance.is_some());
        assert_eq!(d.value, FactValue::String("ready".into()));
    }

    /// F30 regression: the pre-slice opaque shape (all identifier
    /// fields absent) decodes with the lossy `"core"` fallback —
    /// pre-T064 the wire couldn't distinguish Core / Tui / reserved
    /// anyway, so inference can't recover the exact variant. Carries
    /// `value` per the slice-004 wire requirement.
    #[test]
    fn inspection_detail_decodes_legacy_opaque_shape() {
        let legacy = r#"{
            "source_event": "00000000-0000-0000-0000-000000000009",
            "asserted_at_ns": 1000,
            "trace_sequence": 7,
            "value": {"type": "u64", "value": 42}
        }"#;
        let d: InspectionDetail = serde_json::from_str(legacy).expect("decode");
        assert_eq!(d.asserting_kind, "core");
        assert!(d.asserting_behavior.is_none());
        assert!(d.asserting_service.is_none());
        assert_eq!(d.value, FactValue::U64(42));
    }

    // ───────────────────────────────────────────────────────────────────
    // Slice-004 T009A: EventSubscribePattern + SubscribeEvents wire shape.
    // ───────────────────────────────────────────────────────────────────

    #[test]
    fn event_subscribe_pattern_payload_type_json_wire_shape() {
        let p = EventSubscribePattern::PayloadType("buffer-edit".into());
        let s = serde_json::to_string(&p).expect("serialize");
        // Adjacent tag: "type" + "pattern" content field, kebab-case
        // variant name (symmetric with SubscribePattern).
        assert_eq!(s, r#"{"type":"payload-type","pattern":"buffer-edit"}"#);
        let back: EventSubscribePattern = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn event_subscribe_pattern_payload_type_cbor_round_trip() {
        let p = EventSubscribePattern::PayloadType("buffer-open".into());
        let mut buf = Vec::new();
        ciborium::into_writer(&p, &mut buf).expect("encode");
        let back: EventSubscribePattern = ciborium::from_reader(buf.as_slice()).expect("decode");
        assert_eq!(p, back);
    }

    #[test]
    fn event_subscribe_pattern_matches_only_on_payload_type_tag() {
        use crate::provenance::ActorIdentity;
        use crate::types::event::{Event, EventPayload};
        use crate::types::ids::EventId;

        fn fixture(payload: EventPayload) -> Event {
            Event {
                id: EventId::for_testing(0),
                name: "test".into(),
                target: None,
                payload,
                provenance: Provenance::new(ActorIdentity::Core, 0, None).unwrap(),
            }
        }

        let buffer_edit = fixture(EventPayload::BufferEdit {
            entity: crate::types::entity_ref::EntityRef::new(1),
            version: 0,
            edits: vec![],
        });
        let buffer_open = fixture(EventPayload::BufferOpen {
            path: "/tmp/x".into(),
        });

        let edit_pat = EventSubscribePattern::PayloadType("buffer-edit".into());
        assert!(edit_pat.matches(&buffer_edit), "edit pattern matches edit");
        assert!(
            !edit_pat.matches(&buffer_open),
            "edit pattern does NOT match open"
        );

        let open_pat = EventSubscribePattern::PayloadType("buffer-open".into());
        assert!(open_pat.matches(&buffer_open), "open pattern matches open");
        assert!(
            !open_pat.matches(&buffer_edit),
            "open pattern does NOT match edit"
        );

        let unknown = EventSubscribePattern::PayloadType("nonexistent-variant".into());
        assert!(!unknown.matches(&buffer_edit));
        assert!(!unknown.matches(&buffer_open));

        // Slice 005: PayloadTypes(Vec<String>) — multi-tag matcher.
        let buffer_save = fixture(EventPayload::BufferSave {
            entity: crate::types::entity_ref::EntityRef::new(1),
            version: 0,
        });
        let edit_or_save =
            EventSubscribePattern::PayloadTypes(vec!["buffer-edit".into(), "buffer-save".into()]);
        assert!(
            edit_or_save.matches(&buffer_edit),
            "PayloadTypes(edit,save) matches edit"
        );
        assert!(
            edit_or_save.matches(&buffer_save),
            "PayloadTypes(edit,save) matches save"
        );
        assert!(
            !edit_or_save.matches(&buffer_open),
            "PayloadTypes(edit,save) does NOT match open"
        );

        let empty_set = EventSubscribePattern::PayloadTypes(vec![]);
        assert!(
            !empty_set.matches(&buffer_edit),
            "PayloadTypes([]) is the structural never-matches"
        );
    }

    #[test]
    fn event_subscribe_pattern_payload_types_json_wire_shape() {
        // Wire shape under the kebab-case adjacent-tagging convention:
        // `{"type":"payload-types","pattern":["buffer-edit","buffer-save"]}`.
        // Pinned so a future serde-derive change can't silently drift
        // the variant tag or the field name on this surface.
        let p =
            EventSubscribePattern::PayloadTypes(vec!["buffer-edit".into(), "buffer-save".into()]);
        let s = serde_json::to_string(&p).expect("serialize");
        assert_eq!(
            s,
            r#"{"type":"payload-types","pattern":["buffer-edit","buffer-save"]}"#,
        );
        let back: EventSubscribePattern = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn event_subscribe_pattern_payload_types_cbor_round_trip() {
        let p =
            EventSubscribePattern::PayloadTypes(vec!["buffer-edit".into(), "buffer-save".into()]);
        let mut buf = Vec::new();
        ciborium::into_writer(&p, &mut buf).expect("encode");
        let back: EventSubscribePattern = ciborium::from_reader(buf.as_slice()).expect("decode");
        assert_eq!(p, back);
    }

    #[test]
    fn bus_message_subscribe_events_json_wire_shape() {
        let msg =
            BusMessage::SubscribeEvents(EventSubscribePattern::PayloadType("buffer-edit".into()));
        let s = serde_json::to_string(&msg).expect("serialize");
        // BusMessage adjacent tag is "type" + "payload"; kebab-case
        // "subscribe-events" variant name; payload is the
        // EventSubscribePattern wire shape.
        assert_eq!(
            s,
            r#"{"type":"subscribe-events","payload":{"type":"payload-type","pattern":"buffer-edit"}}"#,
        );
        let back: BusMessage = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(msg, back);
    }

    #[test]
    fn bus_message_subscribe_events_cbor_round_trip() {
        let msg =
            BusMessage::SubscribeEvents(EventSubscribePattern::PayloadType("buffer-edit".into()));
        let mut buf = Vec::new();
        ciborium::into_writer(&msg, &mut buf).expect("encode");
        let back: BusMessage = ciborium::from_reader(buf.as_slice()).expect("decode");
        assert_eq!(msg, back);
    }

    // ───────────────────────────────────────────────────────────────────
    // Slice-004 T016A: EventInspect{Request,Response} wire shape.
    // ───────────────────────────────────────────────────────────────────

    #[test]
    fn event_inspect_request_json_wire_shape() {
        let msg = BusMessage::EventInspectRequest {
            request_id: 42,
            event_id: EventId::for_testing(7),
        };
        let s = serde_json::to_string(&msg).expect("serialize");
        // Adjacent tag "event-inspect-request"; payload carries
        // request_id + event_id.
        assert!(
            s.contains("\"type\":\"event-inspect-request\""),
            "expected adjacent tag: {s}"
        );
        assert!(s.contains("\"request_id\":42"), "expected request_id: {s}");
        // Slice 005 §28(a) re-derivation: EventId is now a UUIDv8 hex
        // string on the wire (was u64 in slice 004). The fixture EventId
        // is built via `for_testing(7)` which wraps `Uuid::from_u128(7)`;
        // its hex form is the all-zero UUID with the low 8 bits set to 7.
        assert!(
            s.contains("\"event_id\":\"00000000-0000-0000-0000-000000000007\""),
            "expected event_id: {s}"
        );
        let back: BusMessage = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(msg, back);
    }

    #[test]
    fn event_inspect_response_event_not_found_renders_kebab_case() {
        let msg = BusMessage::EventInspectResponse {
            request_id: 42,
            result: Err(EventInspectionError::EventNotFound),
        };
        let s = serde_json::to_string(&msg).expect("serialize");
        assert!(
            s.contains("\"type\":\"event-inspect-response\""),
            "expected adjacent tag: {s}"
        );
        // EventInspectionError uses kebab-case rename_all per Amendment 5.
        assert!(s.contains("event-not-found"), "expected kebab-case: {s}");
        let back: BusMessage = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(msg, back);
    }

    /// Regression test for the slice-004 `weaver inspect --why` walkback
    /// path. `MAX_EVENT_INGEST_FRAME` must be ≥ `RESPONSE_WRAPPER_HEADROOM`
    /// smaller than `MAX_FRAME_SIZE` so an `Event` that ingests at the
    /// limit can be returned via `EventInspectResponse` without exceeding
    /// the codec's frame ceiling. This pins the headroom invariant: for
    /// any `Event` whose `BusMessage::Event` envelope is ≤
    /// `MAX_EVENT_INGEST_FRAME`, wrapping the same `Event` in
    /// `BusMessage::EventInspectResponse { request_id, result: Ok(_) }`
    /// must still serialise to ≤ `MAX_FRAME_SIZE`.
    ///
    /// Constructs a worst-case event near the ingest ceiling using
    /// `BufferEdit` with a single oversized `new_text` payload (the
    /// realistic vector for hitting the limit — `Vec<TextEdit>` size in
    /// slice 004 is bounded only by the frame limit per spec Q3).
    #[test]
    fn event_inspect_response_fits_within_frame_after_ingest() {
        use crate::bus::codec::{
            MAX_EVENT_INGEST_FRAME, MAX_FRAME_SIZE, RESPONSE_WRAPPER_HEADROOM,
        };
        use crate::provenance::{ActorIdentity, Provenance};
        use crate::types::edit::{Position, Range, TextEdit};
        use crate::types::entity_ref::EntityRef;
        use crate::types::event::EventPayload;
        use crate::types::ids::EventId;

        // Build a BufferEdit Event whose BusMessage::Event encoding lands
        // within a small slack of MAX_EVENT_INGEST_FRAME by binary search
        // over the new_text length. We don't need to hit the limit
        // exactly — any event near it is sufficient to exercise the
        // response-side headroom check.
        let make_event = |new_text_len: usize| -> Event {
            Event {
                id: EventId::for_testing(0),
                name: "buffer/edit".into(),
                target: Some(EntityRef::new(1)),
                payload: EventPayload::BufferEdit {
                    entity: EntityRef::new(1),
                    version: 0,
                    edits: vec![TextEdit {
                        range: Range {
                            start: Position {
                                line: 0,
                                character: 0,
                            },
                            end: Position {
                                line: 0,
                                character: 0,
                            },
                        },
                        new_text: "x".repeat(new_text_len),
                    }],
                },
                provenance: Provenance::new(ActorIdentity::User, 0, None).unwrap(),
            }
        };
        let frame_size = |evt: &Event| -> usize {
            let msg = BusMessage::Event(evt.clone());
            let mut buf = Vec::new();
            ciborium::into_writer(&msg, &mut buf).unwrap();
            buf.len()
        };

        // Binary-search the largest new_text size whose BusMessage::Event
        // frame is ≤ MAX_EVENT_INGEST_FRAME — i.e., the worst case the
        // ingest check accepts.
        let mut lo = 0usize;
        let mut hi = MAX_EVENT_INGEST_FRAME;
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            let size = frame_size(&make_event(mid));
            if size <= MAX_EVENT_INGEST_FRAME {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let event = make_event(lo);
        let ingest_size = frame_size(&event);
        assert!(
            ingest_size <= MAX_EVENT_INGEST_FRAME,
            "ingest worst-case must fit ingest limit: {ingest_size} > {MAX_EVENT_INGEST_FRAME}"
        );

        // Wrap the same Event in EventInspectResponse and verify the
        // response frame still fits MAX_FRAME_SIZE.
        let resp = BusMessage::EventInspectResponse {
            request_id: u64::MAX,
            result: Ok(event),
        };
        let mut resp_buf = Vec::new();
        ciborium::into_writer(&resp, &mut resp_buf).unwrap();
        assert!(
            resp_buf.len() <= MAX_FRAME_SIZE,
            "response frame ({} bytes) must fit MAX_FRAME_SIZE ({MAX_FRAME_SIZE} bytes); wrapper \
             overhead exceeded RESPONSE_WRAPPER_HEADROOM ({RESPONSE_WRAPPER_HEADROOM}): \
             ingest_size={ingest_size}, response_size={}",
            resp_buf.len(),
            resp_buf.len()
        );
    }
}
