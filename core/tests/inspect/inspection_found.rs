//! T049 — scenario test: a fact asserted via behavior firing is
//! inspectable — the returned `InspectionDetail` names the asserting
//! behavior, the source event, a timestamp, and a non-negative trace
//! sequence.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T049.

#[path = "../common/mod.rs"]
mod common;

use common::StubBehavior;
use weaver_core::behavior::dispatcher::Dispatcher;
use weaver_core::fact_space::FactStore;
use weaver_core::inspect::inspect_fact;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{FactKey, FactValue};
use weaver_core::types::ids::{BehaviorId, EventId};

#[tokio::test]
async fn asserted_fact_is_inspectable_with_full_provenance() {
    let entity = EntityRef::new(1);
    let key = FactKey::new(entity, "buffer/dirty");

    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(StubBehavior::new(
        key.clone(),
        FactValue::Bool(true),
    )));

    let event_id = EventId::new(42);
    dispatcher
        .process_event(Event {
            id: event_id,
            name: "buffer/edited".into(),
            target: Some(entity),
            payload: EventPayload::BufferEdited,
            provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
        })
        .await;

    let snapshot = {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        fs.snapshot()
    };
    let trace = dispatcher.trace();
    let trace = trace.lock().await;
    let detail = inspect_fact(&snapshot, &trace, &key).expect("inspection must find asserted fact");

    assert_eq!(detail.source_event, event_id);
    assert_eq!(
        detail.asserting_behavior,
        Some(BehaviorId::new(StubBehavior::ID)),
    );
    // Slice 002: service/instance fields are absent for behavior-authored facts.
    assert!(detail.asserting_service.is_none());
    assert!(detail.asserting_instance.is_none());
    assert!(detail.asserted_at_ns > 0, "asserted_at_ns must be set");
    // trace_sequence is u64; validate it points at a real entry.
    assert!(
        trace.len() as u64 > detail.trace_sequence,
        "trace_sequence must be within the trace log",
    );
}
