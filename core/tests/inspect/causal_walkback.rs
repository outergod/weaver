//! T067a — scenario test: walk the causal chain of a behavior-
//! authored fact back to its originating event, confirming every
//! hop renders structured `ActorIdentity`. Extends with a
//! service-authored step to prove multi-kind chains preserve
//! structured identity at every node.
//!
//! The slice ships no `weaver why` subcommand, so the walkback is
//! manual: given a `FactKey`, `inspect_fact` yields the trace
//! sequence → trace entry → `causal_parent` / `source_event` →
//! upstream trace entry. Each visited entry is required to carry a
//! structured ActorIdentity (never `External(...)`, never missing),
//! and each inspected fact's JSON serialization must carry a
//! non-empty `asserting_kind`.
//!
//! Reference: `specs/002-git-watcher-actor/tasks.md` T067a.

use uuid::Uuid;
use weaver_core::behavior::dirty_tracking::DirtyTrackingBehavior;
use weaver_core::behavior::dispatcher::{Dispatcher, ServicePublishOutcome};
use weaver_core::fact_space::FactStore;
use weaver_core::inspect::inspect_fact;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::trace::entry::TracePayload;
use weaver_core::trace::store::TraceStore;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;

/// Every trace entry must carry a structured ActorIdentity —
/// behavior / service / core / tui / user / host / agent — and no
/// paren-wrapped legacy tag must appear in a serialized form.
fn assert_structured_identity(source: &ActorIdentity) {
    let label = source.kind_label();
    assert!(
        matches!(
            label,
            "behavior" | "service" | "core" | "tui" | "user" | "host" | "agent"
        ),
        "unexpected kind label: {label:?}",
    );
    let json = serde_json::to_string(source).expect("identity serializes");
    assert!(
        !json.contains("\"External("),
        "legacy External(...) tag in ActorIdentity JSON: {json}",
    );
    // Serde's tag field must be present and match the label.
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v.get("type").and_then(|t| t.as_str()),
        Some(label),
        "ActorIdentity `type` tag disagrees with kind_label: {json}",
    );
}

fn assert_inspection_structured(
    snapshot: &weaver_core::fact_space::FactSpaceSnapshot,
    trace: &TraceStore,
    key: &FactKey,
    expected_kind: &str,
) {
    let detail = inspect_fact(snapshot, trace, key).expect("inspect");
    assert_eq!(detail.asserting_kind, expected_kind);
    let json = serde_json::to_value(&detail).expect("serialize");
    assert!(
        json.get("asserting_kind")
            .and_then(|k| k.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "asserting_kind missing or empty on inspection of {key:?}",
    );
}

#[tokio::test]
async fn behavior_chain_walks_back_to_originating_event_with_structured_identity() {
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(DirtyTrackingBehavior::new()));

    // Hop 1: TUI-sourced Event → dispatcher.
    let buffer = EntityRef::new(1);
    let originating_event_id = EventId::new(4242);
    dispatcher
        .process_event(Event {
            id: originating_event_id,
            name: "buffer/edited".into(),
            target: Some(buffer),
            payload: EventPayload::BufferEdited,
            provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
        })
        .await;

    let key = FactKey::new(buffer, "buffer/dirty");
    let snapshot = {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        fs.snapshot()
    };
    let trace_arc = dispatcher.trace();
    let trace = trace_arc.lock().await;

    // Hop A: inspect the fact → returns `asserting_kind = "behavior"`.
    assert_inspection_structured(&snapshot, &trace, &key, "behavior");

    // Hop B: walk back via `source_event`. The inspection detail
    // points at the behavior's triggering event; find that event in
    // the trace and confirm its provenance also renders structured.
    let detail = inspect_fact(&snapshot, &trace, &key).expect("inspect");
    assert_eq!(detail.source_event, originating_event_id);

    // Hop C: every FactAsserted / BehaviorFired / Event trace entry
    // in the store must carry structured identity. Walking the
    // entire trace is the simplest way to prove the invariant
    // across the chain — three entries for this scenario: Event,
    // BehaviorFired, FactAsserted.
    for entry in trace.entries() {
        match &entry.payload {
            TracePayload::Event { event } => {
                assert_structured_identity(&event.provenance.source);
            }
            TracePayload::FactAsserted { fact } => {
                assert_structured_identity(&fact.provenance.source);
            }
            TracePayload::FactRetracted { provenance, .. } => {
                assert_structured_identity(&provenance.source);
            }
            TracePayload::BehaviorFired { .. } | TracePayload::Lifecycle(_) => {
                // No actor identity field on these payloads this
                // slice — the causal chain's identity surfaces
                // via the Event + FactAsserted entries they bracket.
            }
        }
    }
}

#[tokio::test]
async fn multi_kind_chain_preserves_structured_identity_across_service_hop() {
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(DirtyTrackingBehavior::new()));

    // Behavior-authored hop (as above).
    dispatcher
        .process_event(Event {
            id: EventId::new(11),
            name: "buffer/edited".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferEdited,
            provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
        })
        .await;

    // Service-authored hop — simulate the watcher publishing a
    // `repo/dirty` fact on a different entity, on a fresh
    // conn_id so the per-conn identity binding (F14) is clean.
    let repo_entity = EntityRef::new(99);
    let svc =
        ActorIdentity::service("git-watcher", Uuid::new_v4()).expect("valid service identity");
    let outcome = dispatcher
        .publish_from_service(
            42,
            Fact {
                key: FactKey::new(repo_entity, "repo/dirty"),
                value: FactValue::Bool(true),
                provenance: Provenance::new(svc, 200, None).unwrap(),
            },
        )
        .await;
    assert!(matches!(outcome, ServicePublishOutcome::Asserted));

    let snapshot = {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        fs.snapshot()
    };
    let trace_arc = dispatcher.trace();
    let trace = trace_arc.lock().await;

    // Both inspection responses must be structured; the kinds
    // must disagree (one behavior, one service) so we know the
    // walkback distinguishes actor kinds at every hop.
    assert_inspection_structured(
        &snapshot,
        &trace,
        &FactKey::new(EntityRef::new(1), "buffer/dirty"),
        "behavior",
    );
    assert_inspection_structured(
        &snapshot,
        &trace,
        &FactKey::new(repo_entity, "repo/dirty"),
        "service",
    );

    // Every identity-bearing trace entry carries structure across
    // the full multi-kind chain.
    for entry in trace.entries() {
        match &entry.payload {
            TracePayload::Event { event } => {
                assert_structured_identity(&event.provenance.source);
            }
            TracePayload::FactAsserted { fact } => {
                assert_structured_identity(&fact.provenance.source);
            }
            TracePayload::FactRetracted { provenance, .. } => {
                assert_structured_identity(&provenance.source);
            }
            TracePayload::BehaviorFired { .. } | TracePayload::Lifecycle(_) => {}
        }
    }
}
