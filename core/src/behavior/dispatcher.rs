//! Behavior dispatcher — single-VM event processing per L2 P12.
//!
//! Owns the fact space, trace store, and registered behaviors. Events
//! flow through a single mpsc consumer (no inter-behavior parallelism)
//! per `docs/02-architecture.md` §9.4.
//!
//! Slice 001 Phase 2 ships the dispatcher *type* with no behaviors
//! registered — events are appended to the trace and otherwise ignored.
//! The dirty-tracking behavior is registered in Phase 3 (T042).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::bus::delivery::SequenceCounter;
use crate::fact_space::{FactStore, InMemoryFactStore};
use crate::provenance::{ActorIdentity, Provenance};
use crate::trace::entry::{TracePayload, TraceSequence};
use crate::trace::store::TraceStore;
use crate::types::entity_ref::EntityRef;
use crate::types::event::Event;
use crate::types::fact::{Fact, FactKey};
use crate::types::ids::BehaviorId;

/// Outcome of a service-originated fact assertion. Slice-002
/// authority-conflict mechanism (FR-009): first `FactAssert` for a
/// `(family, entity)` pair claims authority on behalf of the
/// connecting client; subsequent asserts from a different
/// `ActorIdentity` on a different connection are rejected.
#[derive(Debug)]
pub enum ServicePublishOutcome {
    Asserted,
    AuthorityConflict {
        family: String,
        entity: EntityRef,
        existing: ActorIdentity,
    },
    /// F14 review fix: once a connection publishes its first fact,
    /// the dispatcher binds that `ActorIdentity` for the lifetime of
    /// the connection. Later publishes that carry a different
    /// identity (same-key re-assert or a new `(family, entity)`
    /// claim) are rejected — otherwise a client could mint
    /// `authority-conflict`-free attribution drift within a single
    /// session and forge audit trails.
    IdentityDrift {
        bound: ActorIdentity,
        attempted: ActorIdentity,
    },
}

/// Outcome of a service-originated fact retraction. A connection may
/// only retract facts it previously asserted; retracting a
/// different-owner fact returns [`Self::NotOwned`]. Retracting a
/// never-asserted fact is an idempotent no-op ([`Self::NotPresent`]).
#[derive(Debug)]
pub enum ServiceRetractOutcome {
    Retracted,
    NotOwned,
    NotPresent,
}

/// Maps `(family_prefix, entity)` → `(connection_id, claiming actor)`.
/// Claims are released when the owning connection drops (listener
/// calls `Dispatcher::release_connection` on disconnect).
#[derive(Default)]
pub(crate) struct AuthorityMap {
    claims: HashMap<(String, EntityRef), (u64, ActorIdentity)>,
}

impl AuthorityMap {
    fn claim(
        &mut self,
        conn_id: u64,
        identity: &ActorIdentity,
        family: &str,
        entity: EntityRef,
    ) -> Result<(), ActorIdentity> {
        let key = (family.to_string(), entity);
        match self.claims.get(&key) {
            None => {
                self.claims.insert(key, (conn_id, identity.clone()));
                Ok(())
            }
            Some((existing_conn, existing_identity)) => {
                // F10 review fix: authority is conn-keyed, NOT
                // identity-keyed. ActorIdentity is client-supplied and
                // visible on the wire in subscribed FactAssert frames —
                // a second connection could otherwise forge the first
                // publisher's identity and bypass FR-009. Only the
                // original connection may re-assert.
                if *existing_conn == conn_id {
                    Ok(())
                } else {
                    Err(existing_identity.clone())
                }
            }
        }
    }

    fn release_connection(&mut self, conn_id: u64) {
        self.claims.retain(|_, (c, _)| *c != conn_id);
    }
}

/// Extract the family prefix from a fact attribute. `"repo/dirty"` →
/// `"repo"`; `"repo/state/on-branch"` → `"repo"`; `"buffer/dirty"` →
/// `"buffer"`. Per `docs/01-system-model.md §2.2`, family is the
/// namespace before the first `/`.
pub(crate) fn family_of(key: &FactKey) -> &str {
    key.attribute.split('/').next().unwrap_or("")
}

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
    /// Slice-002 authority map — tracks `(family, entity) →
    /// (connection, actor)` claims for service-published facts.
    authority: Arc<Mutex<AuthorityMap>>,
    /// Slice-002 connection → set of fact keys it asserted. Paired
    /// with `authority` so `release_connection` can retract every
    /// fact a crashing publisher left behind (F1 review fix) and
    /// `retract_from_service` can reject attempts to delete another
    /// actor's facts (F2 review fix).
    conn_facts: Arc<Mutex<HashMap<u64, HashSet<FactKey>>>>,
    /// Slice-002 connection → first ActorIdentity that ever
    /// published over this connection (F14 review fix). Every
    /// subsequent publish on the same connection must carry the
    /// same identity; any drift is rejected as IdentityDrift.
    /// Cleared on `release_connection`.
    conn_identity: Arc<Mutex<HashMap<u64, ActorIdentity>>>,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self {
            fact_store: Arc::new(Mutex::new(InMemoryFactStore::new())),
            trace: Arc::new(Mutex::new(TraceStore::new())),
            sequence: SequenceCounter::new(),
            behaviors: Vec::new(),
            started_at_ns: now_ns(),
            authority: Arc::new(Mutex::new(AuthorityMap::default())),
            conn_facts: Arc::new(Mutex::new(HashMap::new())),
            conn_identity: Arc::new(Mutex::new(HashMap::new())),
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

impl Dispatcher {
    /// Service-originated fact assertion. Slice 002: external bus
    /// clients (e.g. `weaver-git-watcher`) publish authoritative facts
    /// directly. The fact's provenance carries the originating
    /// [`crate::provenance::ActorIdentity::Service`]; the dispatcher
    /// stores the fact and broadcasts to subscribers.
    ///
    /// Applies the authority-conflict check (FR-009): the first
    /// assertion for a `(family, entity)` pair claims authority on
    /// behalf of `conn_id`; subsequent assertions on a different
    /// connection from a different actor are rejected with
    /// [`ServicePublishOutcome::AuthorityConflict`]. The listener
    /// translates the conflict into a structured `Error` message.
    ///
    /// Appends a `TracePayload::FactAsserted` entry but no
    /// `BehaviorFired` wrapper (there is no behavior firing — the
    /// fact comes straight off the bus). This is what makes the
    /// inspection path's service-branch produce a clean
    /// `InspectionDetail::service(...)` result.
    pub async fn publish_from_service(&self, conn_id: u64, fact: Fact) -> ServicePublishOutcome {
        let family = family_of(&fact.key).to_string();
        let entity = fact.key.entity;
        // F14 review fix: bind one ActorIdentity per connection.
        // First publish records the identity; every subsequent
        // publish on the same conn_id must match. This catches
        // attribution drift that AuthorityMap's conn-keyed claim
        // would otherwise admit — e.g. publishing a fresh
        // (family, entity) under a different identity on the same
        // connection, or re-asserting the same key with a forged
        // source.
        {
            let mut conn_identity = self.conn_identity.lock().await;
            match conn_identity.get(&conn_id) {
                Some(bound) if bound != &fact.provenance.source => {
                    return ServicePublishOutcome::IdentityDrift {
                        bound: bound.clone(),
                        attempted: fact.provenance.source.clone(),
                    };
                }
                Some(_) => {
                    // Matches — proceed without touching the map.
                }
                None => {
                    conn_identity.insert(conn_id, fact.provenance.source.clone());
                }
            }
        }
        {
            let mut auth = self.authority.lock().await;
            if let Err(existing) = auth.claim(conn_id, &fact.provenance.source, &family, entity) {
                return ServicePublishOutcome::AuthorityConflict {
                    family,
                    entity,
                    existing,
                };
            }
        }
        let now = now_ns();
        let _ = self.sequence.next();
        let key = fact.key.clone();
        {
            let mut fact_store = self.fact_store.lock().await;
            let mut trace = self.trace.lock().await;
            trace.append(now, TracePayload::FactAsserted { fact: fact.clone() });
            fact_store.assert(fact);
        }
        // Record ownership only *after* the assert succeeds, so a
        // release-on-disconnect retract never overshoots into keys
        // that weren't actually stored.
        self.conn_facts
            .lock()
            .await
            .entry(conn_id)
            .or_default()
            .insert(key);
        ServicePublishOutcome::Asserted
    }

    /// Service-originated fact retraction. Slice 002: counterpart to
    /// [`Self::publish_from_service`].
    ///
    /// Ownership rule (F2 review fix): a connection may only retract
    /// facts it previously asserted. Attempting to retract a fact
    /// owned by a different connection returns
    /// [`ServiceRetractOutcome::NotOwned`] — the listener surfaces
    /// this as an `Error { category: "not-owner", ... }` to the
    /// offending client. Retracting a fact that isn't currently
    /// asserted (never-published, or already-retracted) is an
    /// idempotent no-op ([`ServiceRetractOutcome::NotPresent`]).
    /// F11 review fix: the retract provenance's **attribution
    /// fields** (`source`, `timestamp_ns`) are synthesized
    /// server-side, NOT accepted from the client. The only piece
    /// the client still contributes is the `causal_parent` event
    /// id — a correlation hint identifying the event that triggered
    /// the retraction, which consumers need to group a retract+assert
    /// pair describing one transition (L2 P11). Everything else would
    /// let a legitimate owner forge attribution (e.g. retract while
    /// claiming to be `ActorIdentity::Core`).
    ///
    /// After ownership is verified via `conn_id`, the dispatcher
    /// looks up the fact's stored `provenance.source` (validated on
    /// the assert path) and attributes the retraction to that actor.
    pub async fn retract_from_service(
        &self,
        conn_id: u64,
        key: FactKey,
        causal_parent: Option<crate::types::ids::EventId>,
    ) -> ServiceRetractOutcome {
        // Ownership check: the retraction is valid only if this
        // connection is the current owner of the key in `conn_facts`.
        {
            let mut conn_facts = self.conn_facts.lock().await;
            match conn_facts.get_mut(&conn_id) {
                Some(set) if set.contains(&key) => {
                    set.remove(&key);
                }
                _ => {
                    // Determine which outcome to return: NotOwned if
                    // the fact is currently asserted (by someone
                    // else), NotPresent if it isn't in the fact store
                    // at all.
                    let present = self.fact_store.lock().await.query(&key).is_some();
                    return if present {
                        ServiceRetractOutcome::NotOwned
                    } else {
                        ServiceRetractOutcome::NotPresent
                    };
                }
            }
        }
        let now = now_ns();
        let _ = self.sequence.next();
        let mut fact_store = self.fact_store.lock().await;
        let mut trace = self.trace.lock().await;
        // Attribute the retraction to the original asserter's identity
        // (which the assert path validated as ActorIdentity::Service
        // and bound to this conn_id). If the fact has vanished between
        // the ownership check and this lookup — shouldn't happen since
        // we hold conn_facts — fall back to Core as a safe default.
        let attributed_source = fact_store
            .query(&key)
            .map(|f| f.provenance.source.clone())
            .unwrap_or(ActorIdentity::Core);
        let synthesized = Provenance::new(attributed_source, now, causal_parent)
            .expect("validated source yields well-formed provenance");
        trace.append(
            now,
            TracePayload::FactRetracted {
                key: key.clone(),
                provenance: synthesized.clone(),
            },
        );
        fact_store.retract(&key, synthesized);
        ServiceRetractOutcome::Retracted
    }

    /// Release every authority claim held by `conn_id` AND retract
    /// every fact it asserted (F1 review fix). Called by the listener
    /// when a bus connection drops — whether cleanly or via crash —
    /// so the fact store doesn't accumulate stale authoritative facts
    /// from dead publishers.
    ///
    /// Synthetic retractions carry `ActorIdentity::Core` as their
    /// source, with a short `<diagnostic-context>` attached via the
    /// trace entry. The original asserter's identity survives in the
    /// trace entry that recorded the assert; the retraction marks
    /// the *core* as the cleanup actor.
    pub async fn release_connection(&self, conn_id: u64) {
        // Extract and drop the ownership map entry first so no
        // concurrent retract_from_service can re-enter the same keys.
        let keys: Vec<FactKey> = {
            let mut conn_facts = self.conn_facts.lock().await;
            conn_facts
                .remove(&conn_id)
                .map(|set| set.into_iter().collect())
                .unwrap_or_default()
        };
        if !keys.is_empty() {
            let mut fact_store = self.fact_store.lock().await;
            let mut trace = self.trace.lock().await;
            for key in keys {
                let prov =
                    Provenance::new(ActorIdentity::Core, now_ns(), None).expect("Core is valid");
                trace.append(
                    now_ns(),
                    TracePayload::FactRetracted {
                        key: key.clone(),
                        provenance: prov.clone(),
                    },
                );
                fact_store.retract(&key, prov);
            }
        }
        // Release the authority claims after the retractions so that
        // a replacement publisher can re-claim without races.
        {
            let mut auth = self.authority.lock().await;
            auth.release_connection(conn_id);
        }
        // Drop the conn-bound identity (F14) so the same conn_id
        // slot — if ever reused by a future monotonic counter
        // wrap — starts fresh.
        self.conn_identity.lock().await.remove(&conn_id);
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

    #[test]
    fn authority_claim_rejects_identity_replay_from_other_connection() {
        // F10 regression: a second connection must not bypass the
        // single-writer guard by forging the first publisher's
        // client-supplied ActorIdentity. Authority is conn-keyed.
        use uuid::Uuid;
        let instance = Uuid::new_v4();
        let identity =
            ActorIdentity::service("git-watcher", instance).expect("valid service identity");
        let entity = EntityRef::new(1);

        let mut map = AuthorityMap::default();
        // Conn A claims.
        assert!(map.claim(1, &identity, "repo", entity).is_ok());
        // Conn A re-asserting the same key is idempotent.
        assert!(map.claim(1, &identity, "repo", entity).is_ok());
        // Conn B replays the same identity → must be rejected.
        let err = map
            .claim(2, &identity, "repo", entity)
            .expect_err("replay must be rejected");
        assert_eq!(err, identity);
        // Conn B with a distinct identity → still rejected, receives
        // the original claimant's identity as diagnostic payload.
        let other = ActorIdentity::service("other-watcher", Uuid::new_v4()).unwrap();
        let err = map
            .claim(2, &other, "repo", entity)
            .expect_err("different identity from other conn must be rejected");
        assert_eq!(err, identity);
    }

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
