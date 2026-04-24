//! F23 regression: when a behavior-asserted fact is overwritten by a
//! service-authored `FactAssert`, `inspect_fact` must return the
//! *service* attribution, not the stale behavior record that
//! `fact_inspection` still carries in its index.
//!
//! The fix derives the inspection shape from the live fact's
//! provenance instead of consulting the behavior index first.

#[path = "../common/mod.rs"]
mod common;

use common::StubBehavior;
use uuid::Uuid;
use weaver_core::behavior::dispatcher::{Dispatcher, ServicePublishOutcome};
use weaver_core::fact_space::FactStore;
use weaver_core::inspect::inspect_fact;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::EventId;

#[tokio::test]
async fn service_overwrite_of_behavior_fact_reports_service_attribution() {
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
            name: "buffer/open".into(),
            target: Some(entity),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
        })
        .await;

    // Sanity: inspect currently reports the stub behavior.
    {
        let snapshot = {
            let fs = dispatcher.fact_store();
            let fs = fs.lock().await;
            fs.snapshot()
        };
        let trace = dispatcher.trace();
        let trace = trace.lock().await;
        let detail = inspect_fact(&snapshot, &trace, &key).expect("behavior fact present");
        assert!(detail.asserting_behavior.is_some());
        assert!(detail.asserting_service.is_none());
    }

    // Overwrite as a service on a distinct connection.
    let instance = Uuid::new_v4();
    let svc_identity =
        ActorIdentity::service("other-service", instance).expect("valid service identity");
    let conn_id: u64 = 1;
    let outcome = dispatcher
        .publish_from_service(
            conn_id,
            Fact {
                key: key.clone(),
                value: FactValue::Bool(false),
                provenance: Provenance::new(svc_identity.clone(), 200, None).unwrap(),
            },
        )
        .await;
    assert!(matches!(outcome, ServicePublishOutcome::Asserted));

    // After the overwrite, inspection must report the service — the
    // stale behavior index entry is still present but no longer
    // authoritative.
    let snapshot = {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        fs.snapshot()
    };
    let trace = dispatcher.trace();
    let trace = trace.lock().await;
    let detail = inspect_fact(&snapshot, &trace, &key).expect("service fact present");

    assert!(
        detail.asserting_behavior.is_none(),
        "stale behavior attribution leaked into overwrite inspection: {detail:?}"
    );
    assert_eq!(detail.asserting_service.as_deref(), Some("other-service"));
    assert_eq!(detail.asserting_instance, Some(instance));
}
