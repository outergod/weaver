//! T038 — scenario test: `BufferEdited` → asserts `buffer/dirty = true`
//! with behavior provenance and causal_parent pointing at the triggering
//! event.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T038.

use weaver_core::behavior::dirty_tracking::DirtyTrackingBehavior;
use weaver_core::behavior::dispatcher::Dispatcher;
use weaver_core::fact_space::FactStore;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{FactKey, FactValue};
use weaver_core::types::ids::{BehaviorId, EventId};

#[tokio::test]
async fn buffer_edited_asserts_buffer_dirty() {
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(DirtyTrackingBehavior::new()));

    let entity = EntityRef::new(1);
    let event_id = EventId::new(42);
    let event = Event {
        id: event_id,
        name: "buffer/edited".into(),
        target: Some(entity),
        payload: EventPayload::BufferEdited,
        provenance: Provenance::new(ActorIdentity::Tui, 1_000, None).unwrap(),
    };

    dispatcher.process_event(event).await;

    let fact_store = dispatcher.fact_store();
    let fact_store = fact_store.lock().await;
    let key = FactKey::new(entity, "buffer/dirty");
    let fact = fact_store
        .query(&key)
        .expect("buffer/dirty must be asserted after BufferEdited");

    assert_eq!(fact.value, FactValue::Bool(true));
    assert_eq!(
        fact.provenance.source,
        ActorIdentity::behavior(BehaviorId::new(DirtyTrackingBehavior::ID)),
    );
    assert_eq!(fact.provenance.causal_parent, Some(event_id));
    assert!(fact.provenance.timestamp_ns > 0);
}
