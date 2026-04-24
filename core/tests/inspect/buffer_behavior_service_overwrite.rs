//! T051 — slice-003 F23 binding: when a behavior-authored
//! `buffer/dirty=true` is overwritten by `weaver-buffers`-authored
//! `buffer/dirty=false`, `inspect_fact` must attribute the fact to
//! the service, not the stale behavior.
//!
//! This is a slice-003-themed sibling of
//! `inspection_overwrite.rs` (which proves the same F23 invariant
//! with a generic `other-service`). Pinning the narrative here keeps
//! the slice-003 authority-handoff contract greppable as the
//! codebase evolves past the slice: a reader asking "does slice-003's
//! behavior→service handoff preserve F23?" finds this file by name.
//!
//! The test constructs both authors manually via the dispatcher's
//! API; `weaver-buffers` the *binary* is not spawned.

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
async fn weaver_buffers_overwrite_of_behavior_buffer_dirty_attributes_to_service() {
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

    // Sanity: before the service overwrite, inspection attributes to
    // the behavior. Establishes the "from" state of the handoff.
    {
        let snapshot = {
            let fs = dispatcher.fact_store();
            let fs = fs.lock().await;
            fs.snapshot()
        };
        let trace = dispatcher.trace();
        let trace = trace.lock().await;
        let detail = inspect_fact(&snapshot, &trace, &key).expect("behavior fact present");
        assert_eq!(detail.asserting_kind.as_str(), "behavior");
        assert!(detail.asserting_behavior.is_some());
        assert!(detail.asserting_service.is_none());
    }

    // weaver-buffers overwrites on its own connection, flipping the
    // value to false.
    let instance = Uuid::new_v4();
    let svc_identity =
        ActorIdentity::service("weaver-buffers", instance).expect("valid service identity");
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

    // After the overwrite, inspection must attribute to weaver-buffers
    // — no stale behavior leak (F23 invariant).
    let snapshot = {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        fs.snapshot()
    };
    let trace = dispatcher.trace();
    let trace = trace.lock().await;
    let detail = inspect_fact(&snapshot, &trace, &key).expect("service fact present");

    assert_eq!(
        detail.asserting_kind.as_str(),
        "service",
        "post-overwrite attribution must be service-kind: {detail:?}"
    );
    assert_eq!(
        detail.asserting_service.as_deref(),
        Some("weaver-buffers"),
        "post-overwrite asserting_service must be weaver-buffers: {detail:?}"
    );
    assert_eq!(
        detail.asserting_instance,
        Some(instance),
        "post-overwrite asserting_instance must match the publish call: {detail:?}"
    );
    assert!(
        detail.asserting_behavior.is_none(),
        "stale behavior attribution leaked through the weaver-buffers overwrite: {detail:?}"
    );
}
