//! Shared helpers for `core/` integration tests.
//!
//! `StubBehavior` is a payload-agnostic behavior used by inspection and
//! property tests that need *some* registered behavior to exercise the
//! `ActorIdentity::Behavior` provenance path. It is intentionally
//! minimal: one configurable target fact is asserted on every fire,
//! with this stub's own `BehaviorId` as provenance.
//!
//! Pulled in via `#[path = "../common/mod.rs"] mod common;` from each
//! test file under `core/tests/`.

#![allow(dead_code)]

use weaver_core::behavior::dispatcher::{Behavior, BehaviorContext, BehaviorOutputs};
use weaver_core::provenance::{ActorIdentity, Provenance};
use weaver_core::types::event::Event;
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::BehaviorId;

/// Test-only behavior that fires on every event and asserts one
/// configurable fact. Provenance is `ActorIdentity::Behavior { id:
/// "test/stub" }` with the firing event's id as `causal_parent`.
pub struct StubBehavior {
    id: BehaviorId,
    target: FactKey,
    value: FactValue,
}

impl StubBehavior {
    pub const ID: &'static str = "test/stub";

    pub fn new(target: FactKey, value: FactValue) -> Self {
        Self {
            id: BehaviorId::new(Self::ID),
            target,
            value,
        }
    }
}

impl Behavior for StubBehavior {
    fn id(&self) -> &BehaviorId {
        &self.id
    }

    fn fire(&self, event: &Event, ctx: BehaviorContext) -> BehaviorOutputs {
        let provenance = Provenance::new(
            ActorIdentity::behavior(self.id.clone()),
            ctx.now_ns,
            Some(event.id),
        )
        .expect("behavior provenance is infallible for valid BehaviorId");
        BehaviorOutputs {
            assertions: vec![Fact {
                key: self.target.clone(),
                value: self.value.clone(),
                provenance,
            }],
            retractions: vec![],
            error: None,
        }
    }
}
