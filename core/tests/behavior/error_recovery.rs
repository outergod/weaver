//! T074 — scenario test: a registered behavior that returns `error: Some(_)`
//! during firing
//!
//! * has its assertions/retractions rolled back (fact-space unchanged),
//! * is recorded as `TracePayload::BehaviorFired { error: Some(_), .. }`,
//! * does not prevent the dispatcher from processing subsequent events.
//!
//! Derived from FR-011 + L2 P3 (defensive host, fault-tolerant guest).
//! Reference: `specs/001-hello-fact/tasks.md` T074.

use weaver_core::behavior::dispatcher::{Behavior, BehaviorContext, BehaviorOutputs, Dispatcher};
use weaver_core::fact_space::FactStore;
use weaver_core::provenance::{Provenance, SourceId};
use weaver_core::trace::entry::TracePayload;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::{BehaviorId, EventId};

/// A behavior that claims to assert a fact but also returns an error — used
/// to verify the dispatcher's atomic-commit contract (no partial outputs
/// on error) and its fault-tolerance (continues with the next event).
struct FixtureErrorBehavior {
    id: BehaviorId,
    tainted_fact: Fact,
}

impl Behavior for FixtureErrorBehavior {
    fn id(&self) -> &BehaviorId {
        &self.id
    }

    fn fire(&self, _event: &Event, _ctx: BehaviorContext) -> BehaviorOutputs {
        BehaviorOutputs {
            assertions: vec![self.tainted_fact.clone()],
            retractions: vec![],
            error: Some("fixture: simulated behavior failure".into()),
        }
    }
}

fn sample_event(id: u64, entity: EntityRef) -> Event {
    Event {
        id: EventId::new(id),
        name: "buffer/edited".into(),
        target: Some(entity),
        payload: EventPayload::BufferEdited,
        provenance: Provenance::new(SourceId::Tui, id.saturating_mul(1_000), None).unwrap(),
    }
}

#[tokio::test]
async fn erroring_behavior_is_contained_and_dispatcher_continues() {
    let fixture_id = BehaviorId::new("test/fixture-error");
    let entity = EntityRef::new(1);
    let tainted_key = FactKey::new(entity, "buffer/dirty");
    let tainted_fact = Fact {
        key: tainted_key.clone(),
        value: FactValue::Bool(true),
        provenance: Provenance::new(SourceId::Behavior(fixture_id.clone()), 42, None).unwrap(),
    };

    let mut dispatcher = Dispatcher::new();
    dispatcher.register(Box::new(FixtureErrorBehavior {
        id: fixture_id.clone(),
        tainted_fact,
    }));

    // 1st event: fixture "fails" mid-firing.
    dispatcher.process_event(sample_event(1, entity)).await;

    // Fact-space must remain empty — the fixture's assertion was rolled back.
    {
        let fs = dispatcher.fact_store();
        let fs = fs.lock().await;
        assert!(
            fs.query(&tainted_key).is_none(),
            "fact-space must be unchanged after an erroring behavior firing",
        );
        assert!(fs.snapshot().is_empty());
    }

    // Trace must record BehaviorFired { error: Some(_), asserted: [] }.
    {
        let trace = dispatcher.trace();
        let trace = trace.lock().await;
        let error_entries: Vec<_> = trace
            .entries()
            .iter()
            .filter_map(|e| match &e.payload {
                TracePayload::BehaviorFired {
                    behavior,
                    asserted,
                    error: Some(err),
                    ..
                } if behavior == &fixture_id => Some((asserted.clone(), err.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            error_entries.len(),
            1,
            "trace must record exactly one BehaviorFired with error for the fixture",
        );
        assert!(
            error_entries[0].0.is_empty(),
            "on-error BehaviorFired entries should not list asserted keys (atomic rollback)",
        );
        assert!(error_entries[0].1.contains("fixture"));
    }

    // 2nd event: dispatcher must continue to fire the fixture normally
    // (it is still registered; it will fail again but the dispatcher
    // should not be tainted).
    dispatcher.process_event(sample_event(2, entity)).await;

    {
        let trace = dispatcher.trace();
        let trace = trace.lock().await;
        let fire_count = trace
            .entries()
            .iter()
            .filter(|e| {
                matches!(
                    &e.payload,
                    TracePayload::BehaviorFired { behavior, .. } if behavior == &fixture_id
                )
            })
            .count();
        assert_eq!(
            fire_count, 2,
            "dispatcher must continue to invoke registered behaviors after an error",
        );
    }
}
