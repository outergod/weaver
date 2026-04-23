//! T051 — property test: for any behavior-asserted fact in the
//! fact-space, `inspect_fact` returns `Ok(InspectionDetail)` whose
//! `asserting_behavior` is non-empty and whose `trace_sequence` indexes
//! an existing trace entry (`< trace.len()`).
//!
//! Reference: `specs/001-hello-fact/tasks.md` T051.

#[path = "../common/mod.rs"]
mod common;

use common::StubBehavior;
use proptest::prelude::*;
use tokio::runtime::Builder;

use weaver_core::behavior::dispatcher::Dispatcher;
use weaver_core::fact_space::FactStore;
use weaver_core::inspect::inspect_fact;
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{FactKey, FactValue};
use weaver_core::types::ids::EventId;

/// Run `event_count` events through a dispatcher registered with
/// `StubBehavior`. Every event triggers the stub (which unconditionally
/// re-asserts the target fact), so the fact is guaranteed present at
/// the end for `event_count >= 1`. Returns the inspection detail's
/// `asserting_behavior` string + `trace_sequence` + `trace.len()`.
fn run(event_count: usize) -> Option<(String, u64, usize)> {
    let rt = Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let entity = EntityRef::new(1);
        let key = FactKey::new(entity, "buffer/dirty");

        let mut d = Dispatcher::new();
        d.register(Box::new(StubBehavior::new(
            key.clone(),
            FactValue::Bool(true),
        )));

        for i in 0..event_count {
            let id = EventId::new((i as u64) + 1);
            d.process_event(Event {
                id,
                name: "buffer/edited".into(),
                target: Some(entity),
                payload: EventPayload::BufferEdited,
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

        match inspect_fact(&snapshot, &trace, &key) {
            Ok(detail) => Some((
                detail
                    .asserting_behavior
                    .as_ref()
                    .map(|b| b.as_str().to_string())
                    .unwrap_or_default(),
                detail.trace_sequence,
                trace.len(),
            )),
            Err(_) => None,
        }
    })
}

proptest! {
    /// For any non-empty event sequence, the stub behavior re-asserts
    /// the target fact on every fire, so inspection must return
    /// `Ok(InspectionDetail)` with a non-empty `asserting_behavior`
    /// and a `trace_sequence` within the trace log.
    #[test]
    fn asserted_fact_always_yields_non_empty_provenance(event_count in 1usize..10) {
        let outcome = run(event_count);
        match outcome {
            Some((behavior, seq, trace_len)) => {
                prop_assert!(!behavior.is_empty(), "asserting_behavior must be non-empty");
                prop_assert!(
                    (seq as usize) < trace_len,
                    "trace_sequence {seq} must index < trace_len {trace_len}",
                );
            }
            None => prop_assert!(false, "event sequence must leave the target fact asserted"),
        }
    }
}
