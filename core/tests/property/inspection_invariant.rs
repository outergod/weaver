//! T051 — property test: for any behavior-asserted fact in the
//! fact-space, `inspect_fact` returns `Ok(InspectionDetail)` whose
//! `asserting_behavior` is non-empty and whose `trace_sequence` indexes
//! an existing trace entry (`< trace.len()`).
//!
//! Reference: `specs/001-hello-fact/tasks.md` T051.

use proptest::prelude::*;
use tokio::runtime::Builder;

use weaver_core::behavior::dirty_tracking::DirtyTrackingBehavior;
use weaver_core::behavior::dispatcher::Dispatcher;
use weaver_core::fact_space::FactStore;
use weaver_core::inspect::inspect_fact;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::FactKey;
use weaver_core::types::ids::EventId;

/// Returns `Some(trace_sequence)` if the inspection succeeds and the
/// invariants hold; `None` if the property should not have applied
/// (empty fact-space after the event sequence).
fn run(events: &[bool]) -> Option<(String, u64, usize)> {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let mut d = Dispatcher::new();
        d.register(Box::new(DirtyTrackingBehavior::new()));
        let entity = EntityRef::new(1);
        for (i, &edit) in events.iter().enumerate() {
            let id = EventId::new((i as u64) + 1);
            let (name, payload) = if edit {
                ("buffer/edited", EventPayload::BufferEdited)
            } else {
                ("buffer/cleaned", EventPayload::BufferCleaned)
            };
            d.process_event(Event {
                id,
                name: name.into(),
                target: Some(entity),
                payload,
                provenance: Provenance::new(ActorIdentity::Tui, (i as u64 + 1) * 1_000, None)
                    .unwrap(),
            })
            .await;
        }
        let snapshot = {
            let fs = d.fact_store();
            let fs = fs.lock().await;
            fs.snapshot()
        };
        let trace = d.trace();
        let trace = trace.lock().await;
        let key = FactKey::new(entity, "buffer/dirty");
        match inspect_fact(&snapshot, &trace, &key) {
            Ok(detail) => Some((
                detail.asserting_behavior.as_str().to_string(),
                detail.trace_sequence,
                trace.len(),
            )),
            Err(_) => None,
        }
    })
}

proptest! {
    #[test]
    fn asserted_fact_always_yields_non_empty_provenance(
        // Guarantee at least one BufferEdited so the final state is likely
        // to have the fact asserted (last_event parity controls it).
        events in proptest::collection::vec(any::<bool>(), 1..10),
    ) {
        // Use the parity invariant: if the last event is BufferEdited,
        // the fact is asserted; we must see Ok.
        let last_edits = events.last().copied().unwrap_or(false);
        let outcome = run(&events);
        match (last_edits, outcome) {
            (true, Some((behavior, seq, trace_len))) => {
                prop_assert!(!behavior.is_empty(), "asserting_behavior must be non-empty");
                prop_assert!(
                    (seq as usize) < trace_len,
                    "trace_sequence {seq} must index < trace_len {trace_len}",
                );
            }
            (true, None) => prop_assert!(false, "last-event-edit should leave fact asserted"),
            (false, Some(_)) => prop_assert!(false, "last-event-clean should leave fact absent"),
            (false, None) => {}
        }
    }
}
