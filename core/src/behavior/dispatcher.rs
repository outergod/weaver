//! Behavior dispatcher — single-VM event processing per L2 P12.
//!
//! Owns the fact space, trace store, and registered behaviors. Events
//! flow through a single mpsc consumer (no inter-behavior parallelism)
//! per `docs/02-architecture.md` §9.4.
//!
//! Slice 001 Phase 2 ships the dispatcher *type* with no behaviors
//! registered — events are appended to the trace and otherwise ignored.
//! The dirty-tracking behavior is registered in Phase 3 (T042).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::bus::delivery::SequenceCounter;
use crate::fact_space::{FactStore, InMemoryFactStore};
use crate::provenance::Provenance;
use crate::trace::entry::{TracePayload, TraceSequence};
use crate::trace::store::TraceStore;
use crate::types::event::Event;
use crate::types::fact::{Fact, FactKey};
use crate::types::ids::BehaviorId;

/// A registered behavior.
///
/// For slice 001 Phase 2 the trait shape is empty (no behaviors are
/// registered). The fleshed-out `Behavior` trait (matches predicate +
/// fire body + outputs) lands alongside the dirty-tracking behavior in
/// Phase 3 (T042).
pub trait Behavior: Send + Sync {
    fn id(&self) -> &BehaviorId;
    /// Run the behavior body. Returns the asserted facts and retracted
    /// keys produced by this firing. Errors short-circuit and are
    /// recorded in the trace; the fact-space is not modified.
    fn fire(&self, event: &Event, ctx: BehaviorContext) -> BehaviorOutputs;
}

/// Context passed to a behavior's `fire` method. Slice 001 Phase 2
/// keeps this minimal; later slices add fact-space query handles,
/// time, RNG, and similar deterministic inputs.
pub struct BehaviorContext {
    pub now_ns: u64,
}

/// Outputs produced by a single behavior firing.
#[derive(Default, Debug)]
pub struct BehaviorOutputs {
    pub assertions: Vec<Fact>,
    pub retractions: Vec<(FactKey, Provenance)>,
    pub error: Option<String>,
}

/// The dispatcher itself. Owned by the runtime in `cli::run_core`.
pub struct Dispatcher {
    fact_store: Arc<Mutex<InMemoryFactStore>>,
    trace: Arc<Mutex<TraceStore>>,
    sequence: SequenceCounter,
    behaviors: Vec<Box<dyn Behavior>>,
    /// Wall-clock nanoseconds the dispatcher was constructed at; used
    /// to compute `uptime_ns` for status responses.
    started_at_ns: u64,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self {
            fact_store: Arc::new(Mutex::new(InMemoryFactStore::new())),
            trace: Arc::new(Mutex::new(TraceStore::new())),
            sequence: SequenceCounter::new(),
            behaviors: Vec::new(),
            started_at_ns: now_ns(),
        }
    }

    pub fn fact_store(&self) -> Arc<Mutex<InMemoryFactStore>> {
        Arc::clone(&self.fact_store)
    }

    pub fn trace(&self) -> Arc<Mutex<TraceStore>> {
        Arc::clone(&self.trace)
    }

    /// Nanoseconds elapsed since the dispatcher was constructed.
    /// Saturates to zero if the system clock ran backwards.
    pub fn uptime_ns(&self) -> u64 {
        now_ns().saturating_sub(self.started_at_ns)
    }

    pub fn register(&mut self, behavior: Box<dyn Behavior>) {
        self.behaviors.push(behavior);
    }

    /// Process one inbound event: append it to the trace, run any
    /// registered behaviors, commit their outputs.
    ///
    /// For slice 001 Phase 2 there are no behaviors registered, so
    /// this only appends the event to the trace.
    pub async fn process_event(&self, event: Event) {
        let now = now_ns();
        let _ = self.sequence.next();

        // Append the event to the trace.
        {
            let mut trace = self.trace.lock().await;
            trace.append(
                now,
                TracePayload::Event {
                    event: event.clone(),
                },
            );
        }

        // Run behaviors. Outputs are committed atomically: when a
        // behavior reports `error: Some(_)`, none of its assertions or
        // retractions are applied. The `BehaviorFired` trace entry is
        // always recorded, with empty `asserted`/`retracted` lists on
        // the error path (P10 — regressions as scenario tests drives
        // this discipline; T074 is the scenario test).
        for behavior in &self.behaviors {
            let ctx = BehaviorContext { now_ns: now };
            let outputs = behavior.fire(&event, ctx);

            let mut fact_store = self.fact_store.lock().await;
            let mut trace = self.trace.lock().await;

            let (asserted_keys, retracted_keys) = if outputs.error.is_some() {
                (Vec::new(), Vec::new())
            } else {
                let asserted_keys: Vec<FactKey> =
                    outputs.assertions.iter().map(|f| f.key.clone()).collect();
                for fact in outputs.assertions {
                    trace.append(now_ns(), TracePayload::FactAsserted { fact: fact.clone() });
                    fact_store.assert(fact);
                }

                let retracted_keys: Vec<FactKey> =
                    outputs.retractions.iter().map(|(k, _)| k.clone()).collect();
                for (key, prov) in outputs.retractions {
                    trace.append(
                        now_ns(),
                        TracePayload::FactRetracted {
                            key: key.clone(),
                            provenance: prov.clone(),
                        },
                    );
                    fact_store.retract(&key, prov);
                }
                (asserted_keys, retracted_keys)
            };

            trace.append(
                now_ns(),
                TracePayload::BehaviorFired {
                    behavior: behavior.id().clone(),
                    triggering_event: event.id,
                    asserted: asserted_keys,
                    retracted: retracted_keys,
                    error: outputs.error,
                },
            );
        }
    }
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Wall-clock nanoseconds. Slice 001 uses `SystemTime` for human-readable
/// timestamps in the trace; a monotonic alternative would be `Instant`,
/// but that's not serializable.
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Suppresses an `unused import` warning while we ship Phase 2 with no
/// behaviors registered.
#[allow(dead_code)]
fn _trace_sequence_used() -> TraceSequence {
    TraceSequence::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ActorIdentity;
    use crate::types::entity_ref::EntityRef;
    use crate::types::event::EventPayload;
    use crate::types::ids::EventId;

    #[tokio::test]
    async fn process_event_with_no_behaviors_appends_to_trace() {
        let d = Dispatcher::new();
        let event = Event {
            id: EventId::new(1),
            name: "buffer/edited".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferEdited,
            provenance: Provenance::new(ActorIdentity::Tui, 100, None).unwrap(),
        };
        d.process_event(event).await;
        let trace_arc = d.trace();
        let trace = trace_arc.lock().await;
        assert_eq!(trace.len(), 1);
    }
}
