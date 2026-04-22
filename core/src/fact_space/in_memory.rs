//! In-memory, HashMap-backed [`FactStore`] implementation.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{FactEvent, FactSpaceSnapshot, FactStore, SubscriptionHandle};
use crate::provenance::Provenance;
use crate::types::fact::{Fact, FactKey};
use crate::types::message::SubscribePattern;

/// In-memory fact store for slice 001.
pub struct InMemoryFactStore {
    facts: HashMap<FactKey, Fact>,
    subscribers: Vec<Subscriber>,
}

struct Subscriber {
    pattern: SubscribePattern,
    tx: mpsc::UnboundedSender<FactEvent>,
}

impl InMemoryFactStore {
    pub fn new() -> Self {
        Self {
            facts: HashMap::new(),
            subscribers: Vec::new(),
        }
    }

    fn broadcast(&mut self, event: FactEvent) {
        let key = match &event {
            FactEvent::Asserted(fact) => &fact.key,
            FactEvent::Retracted { key, .. } => key,
        };
        // Prune subscribers whose channels have been dropped (receiver gone).
        self.subscribers.retain(|sub| {
            if sub.pattern.matches(key) {
                sub.tx.send(event.clone()).is_ok()
            } else {
                // Keep the subscriber — no send, no close check needed.
                true
            }
        });
    }
}

impl Default for InMemoryFactStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FactStore for InMemoryFactStore {
    fn assert(&mut self, fact: Fact) {
        self.facts.insert(fact.key.clone(), fact.clone());
        self.broadcast(FactEvent::Asserted(fact));
    }

    fn retract(&mut self, key: &FactKey, provenance: Provenance) -> Option<Fact> {
        let prev = self.facts.remove(key);
        if prev.is_some() {
            self.broadcast(FactEvent::Retracted {
                key: key.clone(),
                provenance,
            });
        }
        prev
    }

    fn query(&self, key: &FactKey) -> Option<&Fact> {
        self.facts.get(key)
    }

    fn subscribe(&mut self, pattern: SubscribePattern) -> SubscriptionHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subscribers.push(Subscriber { pattern, tx });
        SubscriptionHandle { rx }
    }

    fn snapshot(&self) -> FactSpaceSnapshot {
        Arc::new(self.facts.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ActorIdentity;
    use crate::types::entity_ref::EntityRef;
    use crate::types::fact::FactValue;
    use proptest::prelude::*;

    fn sample_fact(entity: u64, attr: &str, value: bool) -> Fact {
        Fact {
            key: FactKey::new(EntityRef::new(entity), attr),
            value: FactValue::Bool(value),
            provenance: Provenance::new(ActorIdentity::Core, 1000, None).unwrap(),
        }
    }

    fn core_prov() -> Provenance {
        Provenance::new(ActorIdentity::Core, 2000, None).unwrap()
    }

    #[test]
    fn assert_then_query_returns_fact() {
        let mut store = InMemoryFactStore::new();
        let f = sample_fact(1, "buffer/dirty", true);
        store.assert(f.clone());
        assert_eq!(store.query(&f.key), Some(&f));
    }

    #[test]
    fn assert_twice_replaces_value() {
        let mut store = InMemoryFactStore::new();
        let f1 = sample_fact(1, "buffer/dirty", true);
        let f2 = sample_fact(1, "buffer/dirty", false);
        store.assert(f1);
        store.assert(f2.clone());
        assert_eq!(store.query(&f2.key), Some(&f2));
    }

    #[test]
    fn retract_removes_fact_and_returns_prior() {
        let mut store = InMemoryFactStore::new();
        let f = sample_fact(1, "buffer/dirty", true);
        store.assert(f.clone());
        assert_eq!(store.retract(&f.key, core_prov()), Some(f.clone()));
        assert_eq!(store.query(&f.key), None);
    }

    #[test]
    fn retract_missing_returns_none_and_does_not_broadcast() {
        let mut store = InMemoryFactStore::new();
        let key = FactKey::new(EntityRef::new(99), "buffer/dirty");
        // No assertion; retraction should silently no-op.
        assert_eq!(store.retract(&key, core_prov()), None);
    }

    #[tokio::test]
    async fn subscribe_receives_matching_events() {
        let mut store = InMemoryFactStore::new();
        let mut handle = store.subscribe(SubscribePattern::FamilyPrefix("buffer/".into()));

        let f = sample_fact(1, "buffer/dirty", true);
        store.assert(f.clone());
        let evt = handle.rx.recv().await.unwrap();
        match evt {
            FactEvent::Asserted(received) => assert_eq!(received, f),
            _ => panic!("expected Asserted"),
        }

        store.retract(&f.key, core_prov());
        let evt = handle.rx.recv().await.unwrap();
        match evt {
            FactEvent::Retracted { key, .. } => assert_eq!(key, f.key),
            _ => panic!("expected Retracted"),
        }
    }

    #[tokio::test]
    async fn subscriber_filter_family_prefix() {
        let mut store = InMemoryFactStore::new();
        let mut handle = store.subscribe(SubscribePattern::FamilyPrefix("buffer/".into()));

        // Non-matching family — no event.
        store.assert(sample_fact(1, "project/root", true));
        // Matching family — receives event.
        let matching = sample_fact(1, "buffer/dirty", true);
        store.assert(matching.clone());

        let evt = handle.rx.recv().await.unwrap();
        match evt {
            FactEvent::Asserted(received) => assert_eq!(received, matching),
            _ => panic!("expected Asserted(matching)"),
        }
    }

    #[test]
    fn snapshot_mirrors_current_state() {
        let mut store = InMemoryFactStore::new();
        store.assert(sample_fact(1, "buffer/dirty", true));
        store.assert(sample_fact(2, "buffer/dirty", false));
        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
    }

    // T023 {retraction} property test:
    // For any assert/retract sequence, query reflects the final state.
    proptest! {
        #[test]
        fn assert_retract_round_trip_preserves_identity(
            entity in 0u64..100,
            attr in "[a-z]{1,5}/[a-z]{1,8}",
            assert_value in any::<bool>(),
        ) {
            let mut store = InMemoryFactStore::new();
            let f = sample_fact(entity, &attr, assert_value);

            // assert(f) -> retract(key) -> query == None
            store.assert(f.clone());
            prop_assert_eq!(store.query(&f.key), Some(&f));
            let retracted = store.retract(&f.key, core_prov());
            prop_assert_eq!(retracted, Some(f.clone()));
            prop_assert_eq!(store.query(&f.key), None);

            // assert(f) -> assert(f') -> latest-wins
            store.assert(f.clone());
            let f2 = sample_fact(entity, &attr, !assert_value);
            store.assert(f2.clone());
            prop_assert_eq!(store.query(&f.key), Some(&f2));
        }
    }
}
