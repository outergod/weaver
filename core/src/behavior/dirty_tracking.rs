//! Dirty-tracking behavior — the single embedded behavior of slice 001.
//!
//! Fires on `buffer/edited` (asserts `buffer/dirty = true`) and on
//! `buffer/cleaned` (retracts `buffer/dirty`) against the event's
//! target entity. Behavior id: `core/dirty-tracking`.
//!
//! Matching is on the typed [`EventPayload`] variant rather than the
//! wire-stable `name` string — the enum is the Rust face per
//! `specs/001-hello-fact/data-model.md`.

use crate::behavior::dispatcher::{Behavior, BehaviorContext, BehaviorOutputs};
use crate::provenance::{ActorIdentity, Provenance};
use crate::types::event::{Event, EventPayload};
use crate::types::fact::{Fact, FactKey, FactValue};
use crate::types::ids::BehaviorId;

/// The `core/dirty-tracking` behavior.
pub struct DirtyTrackingBehavior {
    id: BehaviorId,
}

impl DirtyTrackingBehavior {
    pub const ID: &'static str = "core/dirty-tracking";
    pub const ATTRIBUTE: &'static str = "buffer/dirty";

    pub fn new() -> Self {
        Self {
            id: BehaviorId::new(Self::ID),
        }
    }

    fn provenance(&self, ctx: &BehaviorContext, event: &Event) -> Provenance {
        // `Provenance::new` is infallible for well-formed ActorIdentity
        // variants; Behavior carries a pre-validated BehaviorId.
        Provenance::new(
            ActorIdentity::behavior(self.id.clone()),
            ctx.now_ns,
            Some(event.id),
        )
        .expect("behavior provenance is infallible")
    }
}

impl Default for DirtyTrackingBehavior {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for DirtyTrackingBehavior {
    fn id(&self) -> &BehaviorId {
        &self.id
    }

    fn fire(&self, event: &Event, ctx: BehaviorContext) -> BehaviorOutputs {
        // The behavior operates on an entity target. Events without a
        // target carry no buffer to mark dirty — skip silently.
        let Some(target) = event.target else {
            return BehaviorOutputs::default();
        };
        let key = FactKey::new(target, Self::ATTRIBUTE);
        let provenance = self.provenance(&ctx, event);

        match event.payload {
            EventPayload::BufferEdited => BehaviorOutputs {
                assertions: vec![Fact {
                    key,
                    value: FactValue::Bool(true),
                    provenance,
                }],
                retractions: vec![],
                error: None,
            },
            EventPayload::BufferCleaned => BehaviorOutputs {
                assertions: vec![],
                retractions: vec![(key, provenance)],
                error: None,
            },
        }
    }
}
