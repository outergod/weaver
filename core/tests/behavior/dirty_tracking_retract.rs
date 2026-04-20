//! T039 {retraction} — scenario test: with `buffer/dirty` pre-asserted,
//! publishing `BufferCleaned` retracts it.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T039.

use weaver_core::behavior::dirty_tracking::DirtyTrackingBehavior;
use weaver_core::behavior::dispatcher::Dispatcher;
use weaver_core::fact_space::FactStore;
use weaver_core::provenance::{Provenance, SourceId};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::FactKey;
use weaver_core::types::ids::EventId;

#[tokio::test]
async fn buffer_cleaned_retracts_buffer_dirty() {
    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(DirtyTrackingBehavior::new()));

    let entity = EntityRef::new(1);

    // Seed: assert buffer/dirty via a BufferEdited event.
    let edited = Event {
        id: EventId::new(1),
        name: "buffer/edited".into(),
        target: Some(entity),
        payload: EventPayload::BufferEdited,
        provenance: Provenance::new(SourceId::Tui, 1_000, None).unwrap(),
    };
    dispatcher.process_event(edited).await;

    let key = FactKey::new(entity, "buffer/dirty");
    {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        assert!(
            fs.query(&key).is_some(),
            "seed: buffer/dirty must be asserted before the retraction path is exercised",
        );
    }

    // Retract: publish BufferCleaned.
    let cleaned = Event {
        id: EventId::new(2),
        name: "buffer/cleaned".into(),
        target: Some(entity),
        payload: EventPayload::BufferCleaned,
        provenance: Provenance::new(SourceId::Tui, 2_000, None).unwrap(),
    };
    dispatcher.process_event(cleaned).await;

    let fs = dispatcher.fact_store();
    let fs = fs.lock().await;
    assert!(
        fs.query(&key).is_none(),
        "buffer/dirty must be retracted after BufferCleaned",
    );
}
