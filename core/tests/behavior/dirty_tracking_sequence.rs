//! T040 — scenario test: an `Edit → Clean → Edit` sequence leaves
//! `buffer/dirty` asserted at the end (last-event wins).
//!
//! Reference: `specs/001-hello-fact/tasks.md` T040.

use weaver_core::behavior::dirty_tracking::DirtyTrackingBehavior;
use weaver_core::behavior::dispatcher::Dispatcher;
use weaver_core::fact_space::FactStore;
use weaver_core::provenance::{Provenance, SourceId};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::FactKey;
use weaver_core::types::ids::EventId;

fn edit(entity: EntityRef, id: u64) -> Event {
    Event {
        id: EventId::new(id),
        name: "buffer/edited".into(),
        target: Some(entity),
        payload: EventPayload::BufferEdited,
        provenance: Provenance::new(SourceId::Tui, id * 1_000, None).unwrap(),
    }
}

fn clean(entity: EntityRef, id: u64) -> Event {
    Event {
        id: EventId::new(id),
        name: "buffer/cleaned".into(),
        target: Some(entity),
        payload: EventPayload::BufferCleaned,
        provenance: Provenance::new(SourceId::Tui, id * 1_000, None).unwrap(),
    }
}

#[tokio::test]
async fn edit_clean_edit_ends_with_buffer_dirty_asserted() {
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(DirtyTrackingBehavior::new()));

    let entity = EntityRef::new(1);
    dispatcher.process_event(edit(entity, 1)).await;
    dispatcher.process_event(clean(entity, 2)).await;
    dispatcher.process_event(edit(entity, 3)).await;

    let fs = dispatcher.fact_store();
    let fs = fs.lock().await;
    let key = FactKey::new(entity, "buffer/dirty");
    let fact = fs
        .query(&key)
        .expect("final state after Edit→Clean→Edit: buffer/dirty must be asserted");
    // The assertion comes from the third event.
    assert_eq!(fact.provenance.causal_parent, Some(EventId::new(3)));
}
