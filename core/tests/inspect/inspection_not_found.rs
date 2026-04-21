//! T050 — scenario test: inspecting a fact that is not currently
//! asserted returns `InspectionError::FactNotFound`.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T050.

use weaver_core::fact_space::{FactStore, InMemoryFactStore};
use weaver_core::inspect::inspect_fact;
use weaver_core::trace::store::TraceStore;
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::fact::FactKey;
use weaver_core::types::message::InspectionError;

#[test]
fn empty_fact_space_returns_fact_not_found() {
    let store = InMemoryFactStore::new();
    let snapshot = store.snapshot();
    let trace = TraceStore::new();
    let key = FactKey::new(EntityRef::new(1), "buffer/dirty");

    let result = inspect_fact(&snapshot, &trace, &key);
    assert_eq!(result, Err(InspectionError::FactNotFound));
}
