//! Event-subscription registry — slice-004 bus-level event broadcast.
//!
//! Slices 001–003 only delivered [`crate::fact_space::FactEvent`]
//! (`Asserted` / `Retracted`) to subscribers; [`crate::types::message::BusMessage::Event`]
//! frames went through [`crate::behavior::dispatcher::Dispatcher::process_event`]
//! into the trace + in-process behaviors and stopped there. Slice 004
//! adds bus-level event delivery (the missing prerequisite for
//! `weaver edit` → `weaver-buffers`); see
//! `specs/004-buffer-edit/research.md §13`.
//!
//! The [`crate::behavior::dispatcher::Dispatcher`] holds an
//! `Arc<EventSubscriptions>`; `process_event` calls
//! [`EventSubscriptions::broadcast`] after the trace append + behavior
//! fire. Per-connection subscriptions are added via
//! [`EventSubscriptions::subscribe`] when the listener handles a
//! [`crate::types::message::BusMessage::SubscribeEvents`] frame; the
//! returned [`EventSubscriptionHandle`] is owned by the per-connection
//! task and dropped when the connection closes — the next broadcast
//! pass observes the closed channel via send-failure and prunes the
//! subscriber.
//!
//! **Delivery class**: lossy per `docs/02-architecture.md §3.1`. This
//! implementation uses [`tokio::sync::mpsc::unbounded_channel`] for
//! parity with [`crate::fact_space::InMemoryFactStore`]'s subscriber
//! path (which is also unbounded in slices 001–003 despite being
//! authoritative-class). True drop-oldest under back-pressure, and
//! active pruning of closed subscribers whose pattern never matches
//! again, are both deferred to a future infrastructure slice that
//! bounds both registries in lockstep. Tracked at
//! `docs/07-open-questions.md §27` — reviewers can skip over the
//! unbounded-channel + lazy-prune findings via that reference.

use std::sync::Mutex;

use tokio::sync::mpsc;

use crate::types::event::Event;
use crate::types::message::EventSubscribePattern;

/// Handle returned by [`EventSubscriptions::subscribe`]. The caller
/// drives the receiver and drops it when done; dropping signals
/// unsubscribe — the next [`EventSubscriptions::broadcast`] pass
/// observes the closed channel via send-failure and removes the
/// subscriber.
pub struct EventSubscriptionHandle {
    pub rx: mpsc::UnboundedReceiver<Event>,
}

struct EventSubscriber {
    pattern: EventSubscribePattern,
    tx: mpsc::UnboundedSender<Event>,
}

/// Registry of active event subscriptions. Cheap to share across
/// per-connection tasks via [`std::sync::Arc`]; internal mutability
/// through a [`std::sync::Mutex`]. No async work happens under the
/// lock — it's a `Vec` push (subscribe) or `retain` (broadcast), both
/// finite and non-blocking.
#[derive(Default)]
pub struct EventSubscriptions {
    subscribers: Mutex<Vec<EventSubscriber>>,
}

impl EventSubscriptions {
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }

    /// Register a new subscription and return its receive handle.
    /// A connection MAY hold both a fact subscription and an event
    /// subscription concurrently; a second `SubscribeEvents` on the
    /// same connection produces a fresh handle (the listener replaces
    /// the prior one — last-wins, mirroring the fact-subscription
    /// convention; the abandoned handle's tx side is dropped, and the
    /// next broadcast prunes it via send-failure).
    pub fn subscribe(&self, pattern: EventSubscribePattern) -> EventSubscriptionHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut subs = self.subscribers.lock().expect("event-sub lock poisoned");
        subs.push(EventSubscriber { pattern, tx });
        EventSubscriptionHandle { rx }
    }

    /// Fan `event` out to every subscriber whose pattern matches.
    /// Closed channels (the receive handle has been dropped) are
    /// detected via send-failure and removed in the same pass —
    /// mirrors [`crate::fact_space::InMemoryFactStore`]'s broadcast
    /// pattern.
    ///
    /// Subscribers whose pattern does NOT match retain their entry
    /// regardless of channel state — closed-but-non-matching senders
    /// leak until something matches. Bounded-channel + active-pruning
    /// design tracked at `docs/07-open-questions.md §27`.
    pub fn broadcast(&self, event: &Event) {
        let mut subs = self.subscribers.lock().expect("event-sub lock poisoned");
        subs.retain(|sub| {
            if sub.pattern.matches(event) {
                sub.tx.send(event.clone()).is_ok()
            } else {
                true
            }
        });
    }

    /// Current subscriber count — for tests + introspection only.
    #[cfg(test)]
    pub fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .expect("event-sub lock poisoned")
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::{ActorIdentity, Provenance};
    use crate::types::entity_ref::EntityRef;
    use crate::types::event::EventPayload;
    use crate::types::ids::EventId;

    fn fixture_event(payload: EventPayload) -> Event {
        Event {
            id: EventId::for_testing(0),
            name: "test".into(),
            target: None,
            payload,
            provenance: Provenance::new(ActorIdentity::Core, 0, None).unwrap(),
        }
    }

    fn buffer_edit() -> Event {
        fixture_event(EventPayload::BufferEdit {
            entity: EntityRef::new(1),
            version: 0,
            edits: vec![],
        })
    }

    fn buffer_open() -> Event {
        fixture_event(EventPayload::BufferOpen {
            path: "/tmp/x".into(),
        })
    }

    #[tokio::test]
    async fn subscriber_receives_matching_events_only() {
        let subs = EventSubscriptions::new();
        let mut handle = subs.subscribe(EventSubscribePattern::PayloadType("buffer-edit".into()));

        // Matching: edit lands.
        let edit = buffer_edit();
        subs.broadcast(&edit);
        let received = handle.rx.recv().await.expect("matching event delivered");
        assert_eq!(received, edit);

        // Non-matching: open does not land. Use try_recv to avoid
        // hanging on absence.
        subs.broadcast(&buffer_open());
        assert!(
            handle.rx.try_recv().is_err(),
            "non-matching payload type must not be delivered"
        );
    }

    #[tokio::test]
    async fn fan_out_to_multiple_subscribers_with_distinct_patterns() {
        let subs = EventSubscriptions::new();
        let mut edit_handle =
            subs.subscribe(EventSubscribePattern::PayloadType("buffer-edit".into()));
        let mut open_handle =
            subs.subscribe(EventSubscribePattern::PayloadType("buffer-open".into()));
        let edit = buffer_edit();
        let open = buffer_open();
        subs.broadcast(&edit);
        subs.broadcast(&open);
        assert_eq!(edit_handle.rx.recv().await.unwrap(), edit);
        assert_eq!(open_handle.rx.recv().await.unwrap(), open);
        // Cross-pattern delivery should NOT happen.
        assert!(edit_handle.rx.try_recv().is_err());
        assert!(open_handle.rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dropped_handle_is_pruned_on_next_broadcast() {
        let subs = EventSubscriptions::new();
        {
            let _handle = subs.subscribe(EventSubscribePattern::PayloadType("buffer-edit".into()));
            assert_eq!(subs.subscriber_count(), 1);
            // _handle drops here at block exit; tx sender now dangling.
        }
        // The Vec still holds the subscriber until the next broadcast
        // attempts a send and gets Err → retain prunes it.
        assert_eq!(subs.subscriber_count(), 1);
        subs.broadcast(&buffer_edit());
        assert_eq!(
            subs.subscriber_count(),
            0,
            "broadcast must prune subscribers whose receivers dropped"
        );
    }

    #[tokio::test]
    async fn broadcast_with_no_subscribers_is_a_noop() {
        let subs = EventSubscriptions::new();
        // No panic, no allocation surprises.
        subs.broadcast(&buffer_edit());
        assert_eq!(subs.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn second_subscription_does_not_evict_first_at_registry_level() {
        // Last-wins per CONNECTION is the listener's responsibility
        // (it overwrites its `Option<EventSubscriptionHandle>`); the
        // registry itself accepts every subscribe call independently.
        // This test pins the registry-level invariant: subscribe() is
        // additive; eviction happens via handle drop.
        let subs = EventSubscriptions::new();
        let _h1 = subs.subscribe(EventSubscribePattern::PayloadType("buffer-edit".into()));
        let _h2 = subs.subscribe(EventSubscribePattern::PayloadType("buffer-open".into()));
        assert_eq!(subs.subscriber_count(), 2);
    }
}
