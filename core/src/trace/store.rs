//! Append-only trace store with reverse causal index.
//!
//! Snapshot-and-truncate retention (arch §10.2) is a future concern;
//! slice 001 keeps the full in-memory log.

use std::collections::HashMap;

use super::entry::{TraceEntry, TracePayload, TraceSequence};
use crate::types::fact::FactKey;
use crate::types::ids::EventId;

/// Append-only trace log with reverse causal index.
///
/// The indexes support `O(1)` lookup from an `EventId` or `FactKey` to
/// the trace sequence where that item was recorded, enabling
/// `O(path length)` `why?` walk-back per `docs/02-architecture.md` §10.1.
pub struct TraceStore {
    entries: Vec<TraceEntry>,
    next_sequence: TraceSequence,
    /// `EventId → TraceSequence` for events that have been recorded.
    by_event: HashMap<EventId, TraceSequence>,
    /// `FactKey → most-recent TraceSequence` where the fact was
    /// asserted or retracted. A later assert/retract overwrites.
    by_fact: HashMap<FactKey, TraceSequence>,
    /// `FactKey → sequence where the fact was *asserted*`. This is
    /// distinct from `by_fact` because retractions overwrite the
    /// general index; inspection wants the asserting firing.
    by_fact_assert: HashMap<FactKey, TraceSequence>,
    /// `FactKey → (triggering EventId, BehaviorId)` for the most recent
    /// behavior firing that asserted the fact. Populated by
    /// `BehaviorFired` entries.
    by_fact_asserting_behavior: HashMap<FactKey, (EventId, crate::types::ids::BehaviorId)>,
}

impl TraceStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_sequence: TraceSequence::ZERO,
            by_event: HashMap::new(),
            by_fact: HashMap::new(),
            by_fact_assert: HashMap::new(),
            by_fact_asserting_behavior: HashMap::new(),
        }
    }

    /// Append a trace entry and return its sequence.
    pub fn append(&mut self, timestamp_ns: u64, payload: TracePayload) -> TraceSequence {
        let seq = self.next_sequence;
        self.update_indexes(seq, &payload);
        self.entries.push(TraceEntry {
            sequence: seq,
            timestamp_ns,
            payload,
        });
        self.next_sequence = self.next_sequence.next();
        seq
    }

    fn update_indexes(&mut self, seq: TraceSequence, payload: &TracePayload) {
        match payload {
            TracePayload::Event { event } => {
                self.by_event.insert(event.id, seq);
            }
            TracePayload::FactAsserted { fact } => {
                self.by_fact.insert(fact.key.clone(), seq);
                self.by_fact_assert.insert(fact.key.clone(), seq);
            }
            TracePayload::FactRetracted { key, .. } => {
                self.by_fact.insert(key.clone(), seq);
                self.by_fact_assert.remove(key);
                self.by_fact_asserting_behavior.remove(key);
            }
            TracePayload::BehaviorFired {
                behavior,
                triggering_event,
                asserted,
                retracted,
                ..
            } => {
                // A behavior firing may have asserted and/or retracted facts.
                for key in asserted {
                    self.by_fact_asserting_behavior
                        .insert(key.clone(), (*triggering_event, behavior.clone()));
                }
                for key in retracted {
                    self.by_fact_asserting_behavior.remove(key);
                }
            }
            TracePayload::Lifecycle(_) => {}
        }
    }

    pub fn get(&self, seq: TraceSequence) -> Option<&TraceEntry> {
        self.entries.get(seq.as_u64() as usize)
    }

    /// Returns the trace sequence of the event with the given `EventId`,
    /// or `None` if no such event has been recorded.
    ///
    /// **Caveat**: `EventId` is unique per producer, not globally
    /// (`core/src/types/ids.rs`). The `by_event` index is last-writer-
    /// wins on collision, so this lookup may return a different
    /// producer's later event when IDs collide. `weaver inspect --why`
    /// is the user-visible consumer; mitigation tracked at
    /// `docs/07-open-questions.md §28`.
    pub fn find_event(&self, id: EventId) -> Option<TraceSequence> {
        self.by_event.get(&id).copied()
    }

    /// Most recent trace sequence touching the given fact key (assert
    /// or retract). Returns `None` if the fact has never appeared.
    pub fn find_fact(&self, key: &FactKey) -> Option<TraceSequence> {
        self.by_fact.get(key).copied()
    }

    /// Trace sequence where the fact is currently asserted. Returns
    /// `None` if the fact is not currently asserted.
    pub fn find_fact_assert(&self, key: &FactKey) -> Option<TraceSequence> {
        self.by_fact_assert.get(key).copied()
    }

    /// For a currently-asserted fact, the triggering event + asserting
    /// behavior. Used by the inspection handler (FR-008) to build
    /// `InspectionDetail` responses.
    pub fn fact_inspection(
        &self,
        key: &FactKey,
    ) -> Option<(EventId, crate::types::ids::BehaviorId, TraceSequence)> {
        let (event_id, behavior) = self.by_fact_asserting_behavior.get(key)?;
        let seq = *self.by_fact_assert.get(key)?;
        Some((*event_id, behavior.clone(), seq))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Read-only view of all trace entries in append order. Intended for
    /// tests and for the inspection handler's fall-back scans — the
    /// reverse indexes are the primary access path.
    pub fn entries(&self) -> &[TraceEntry] {
        &self.entries
    }
}

impl Default for TraceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{ActorIdentity, Provenance};
    use crate::trace::entry::TracePayload;
    use crate::types::entity_ref::EntityRef;
    use crate::types::event::{Event, EventPayload};
    use crate::types::fact::{Fact, FactValue};
    use crate::types::ids::BehaviorId;

    #[test]
    fn append_and_lookup_event() {
        let mut store = TraceStore::new();
        let event = Event {
            id: EventId::new(42),
            name: "buffer/open".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
        };
        let seq = store.append(
            100,
            TracePayload::Event {
                event: event.clone(),
            },
        );
        assert_eq!(store.find_event(event.id), Some(seq));
        assert_eq!(store.get(seq).unwrap().sequence, seq);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn fact_inspection_records_asserting_behavior() {
        let mut store = TraceStore::new();
        let behavior = BehaviorId::new("core/dirty-tracking");
        let event_id = EventId::new(42);
        let fact_key = FactKey::new(EntityRef::new(1), "buffer/dirty");

        // Event
        store.append(
            100,
            TracePayload::Event {
                event: Event {
                    id: event_id,
                    name: "buffer/open".into(),
                    target: Some(EntityRef::new(1)),
                    payload: EventPayload::BufferOpen {
                        path: "/tmp/weaver-fixture".into(),
                    },
                    provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
                },
            },
        );
        // Fact assertion
        let fact = Fact {
            key: fact_key.clone(),
            value: FactValue::Bool(true),
            provenance: Provenance::new(
                ActorIdentity::behavior(behavior.clone()),
                200,
                Some(event_id),
            )
            .unwrap(),
        };
        let assert_seq = store.append(200, TracePayload::FactAsserted { fact });
        // Behavior firing record
        store.append(
            201,
            TracePayload::BehaviorFired {
                behavior: behavior.clone(),
                triggering_event: event_id,
                asserted: vec![fact_key.clone()],
                retracted: vec![],
                error: None,
            },
        );

        let (ev, bid, seq) = store.fact_inspection(&fact_key).unwrap();
        assert_eq!(ev, event_id);
        assert_eq!(bid, behavior);
        assert_eq!(seq, assert_seq);
    }

    #[test]
    fn retraction_clears_fact_inspection() {
        let mut store = TraceStore::new();
        let behavior = BehaviorId::new("core/dirty-tracking");
        let event_id = EventId::new(42);
        let fact_key = FactKey::new(EntityRef::new(1), "buffer/dirty");

        let fact = Fact {
            key: fact_key.clone(),
            value: FactValue::Bool(true),
            provenance: Provenance::new(
                ActorIdentity::behavior(behavior.clone()),
                200,
                Some(event_id),
            )
            .unwrap(),
        };
        store.append(200, TracePayload::FactAsserted { fact });
        store.append(
            201,
            TracePayload::BehaviorFired {
                behavior: behavior.clone(),
                triggering_event: event_id,
                asserted: vec![fact_key.clone()],
                retracted: vec![],
                error: None,
            },
        );
        assert!(store.fact_inspection(&fact_key).is_some());

        store.append(
            300,
            TracePayload::FactRetracted {
                key: fact_key.clone(),
                provenance: Provenance::new(ActorIdentity::Core, 300, None).unwrap(),
            },
        );
        assert!(store.fact_inspection(&fact_key).is_none());
        // `find_fact` still resolves — to the retraction.
        assert!(store.find_fact(&fact_key).is_some());
    }
}
