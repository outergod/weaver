//! T068 — property test: for any `InspectionDetail` value of a
//! slice-emitted kind, the JSON emitter + serde round-trip
//! preserves every field; the actor identity is unambiguously
//! reconstructible from the emitted shape.
//!
//! Scoped to the four kinds this slice actually emits — `behavior`,
//! `service`, `core`, `tui`. The reserved variants (`user`, `host`,
//! `agent`) aren't inspected against real producers this slice;
//! their richer payload is a future-slice concern per Phase-4
//! design Option C.
//!
//! Reference: `specs/002-git-watcher-actor/tasks.md` T068.

use proptest::prelude::*;
use uuid::Uuid;
use weaver_core::types::ids::{BehaviorId, EventId};
use weaver_core::types::message::InspectionDetail;

fn arb_event_id() -> impl Strategy<Value = EventId> {
    any::<u64>().prop_map(EventId::new)
}

fn arb_asserted_at_ns() -> impl Strategy<Value = u64> {
    any::<u64>()
}

fn arb_trace_sequence() -> impl Strategy<Value = u64> {
    any::<u64>()
}

fn arb_behavior_id() -> impl Strategy<Value = BehaviorId> {
    // Behavior ids follow `family/name` where both segments are
    // non-empty kebab-ish strings. Mirror the dispatcher's real-
    // world usage without pinning a narrow alphabet.
    (
        proptest::collection::vec("[a-z0-9]{1,6}", 1..=3),
        proptest::collection::vec("[a-z0-9]{1,6}", 1..=3),
    )
        .prop_map(|(fam, name)| {
            let id = format!("{}/{}", fam.join("-"), name.join("-"));
            BehaviorId::new(id)
        })
}

fn arb_service_id() -> impl Strategy<Value = String> {
    // Kebab-case: lowercase + digits + internal hyphens (no
    // leading/trailing/consecutive hyphens). Matches
    // `ActorIdentity::service`'s validator so produced ids are
    // structurally valid.
    proptest::collection::vec("[a-z0-9]{1,6}", 1..=4).prop_map(|parts| parts.join("-"))
}

fn arb_detail_behavior() -> impl Strategy<Value = InspectionDetail> {
    (
        arb_event_id(),
        arb_behavior_id(),
        arb_asserted_at_ns(),
        arb_trace_sequence(),
    )
        .prop_map(|(ev, bid, ts, seq)| InspectionDetail::behavior(ev, bid, ts, seq))
}

fn arb_detail_service() -> impl Strategy<Value = InspectionDetail> {
    (
        arb_event_id(),
        arb_service_id(),
        any::<u128>().prop_map(Uuid::from_u128),
        arb_asserted_at_ns(),
        arb_trace_sequence(),
    )
        .prop_map(|(ev, svc, inst, ts, seq)| InspectionDetail::service(ev, svc, inst, ts, seq))
}

fn arb_detail_kind_only() -> impl Strategy<Value = InspectionDetail> {
    prop_oneof![Just("core"), Just("tui")].prop_flat_map(|kind| {
        (arb_event_id(), arb_asserted_at_ns(), arb_trace_sequence())
            .prop_map(move |(ev, ts, seq)| InspectionDetail::kind_only(kind, ev, ts, seq))
    })
}

proptest! {
    /// Any emitted-this-slice `InspectionDetail` round-trips through
    /// JSON without loss. This is the property-level evidence for
    /// T067's "never renders opaque" claim: the `asserting_kind`
    /// discriminator survives serialization for every variant.
    #[test]
    fn detail_round_trips_through_json(
        detail in prop_oneof![
            arb_detail_behavior(),
            arb_detail_service(),
            arb_detail_kind_only(),
        ],
    ) {
        let json = serde_json::to_string(&detail).expect("serialize");
        let back: InspectionDetail =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(detail, back);
    }

    /// `asserting_kind` is always present and carries one of the
    /// four values the slice emits. No response leaks a legacy
    /// `External(...)` or otherwise-opaque marker into the field.
    #[test]
    fn asserting_kind_is_always_a_valid_slice_label(
        detail in prop_oneof![
            arb_detail_behavior(),
            arb_detail_service(),
            arb_detail_kind_only(),
        ],
    ) {
        let json = serde_json::to_string(&detail).expect("serialize");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        let kind = v
            .get("asserting_kind")
            .and_then(|k| k.as_str())
            .expect("asserting_kind present");
        prop_assert!(
            matches!(kind, "behavior" | "service" | "core" | "tui"),
            "asserting_kind {kind:?} must be a slice-emitted label",
        );
        prop_assert!(
            !kind.contains('('),
            "asserting_kind must never encode a paren-wrapped opaque tag",
        );
    }
}
