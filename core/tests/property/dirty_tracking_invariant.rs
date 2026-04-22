//! T041 — property test: for any sequence of `BufferEdited`/`BufferCleaned`
//! events, the presence of `buffer/dirty` at the end matches the type of
//! the last event (Edited → asserted, Cleaned → absent). An empty
//! sequence leaves `buffer/dirty` absent.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T041.

use proptest::prelude::*;
use tokio::runtime::Builder;

use weaver_core::behavior::dirty_tracking::DirtyTrackingBehavior;
use weaver_core::behavior::dispatcher::Dispatcher;
use weaver_core::fact_space::FactStore;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::FactKey;
use weaver_core::types::ids::EventId;

/// `true` = BufferEdited; `false` = BufferCleaned.
fn run_sequence(events: &[bool]) -> bool {
    let rt = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio current-thread runtime");
    rt.block_on(async {
        let mut dispatcher = Dispatcher::new();
        dispatcher.register(Box::new(DirtyTrackingBehavior::new()));
        let entity = EntityRef::new(1);
        for (i, &is_edit) in events.iter().enumerate() {
            let id = EventId::new((i as u64) + 1);
            let (name, payload) = if is_edit {
                ("buffer/edited", EventPayload::BufferEdited)
            } else {
                ("buffer/cleaned", EventPayload::BufferCleaned)
            };
            let event = Event {
                id,
                name: name.into(),
                target: Some(entity),
                payload,
                provenance: Provenance::new(ActorIdentity::Tui, (i as u64 + 1) * 1_000, None)
                    .unwrap(),
            };
            dispatcher.process_event(event).await;
        }
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        fs.query(&FactKey::new(entity, "buffer/dirty")).is_some()
    })
}

proptest! {
    // Shrinking-friendly Vec of 0..10 events.
    #[test]
    fn buffer_dirty_tracks_last_event_parity(events in proptest::collection::vec(any::<bool>(), 0..10)) {
        let asserted = run_sequence(&events);
        let expected = events.last().copied().unwrap_or(false);
        prop_assert_eq!(
            asserted, expected,
            "events={:?} expected asserted={} but got {}",
            events, expected, asserted,
        );
    }
}
