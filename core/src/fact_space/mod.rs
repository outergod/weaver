//! Fact space — `FactStore` trait and in-memory implementation.
//!
//! ECS-library decision (Bevy / Hecs / Flecs / custom archetype) is
//! intentionally deferred — see `specs/001-hello-fact/research.md` §13.
//! For Hello-fact the trait is backed by a `HashMap<FactKey, Fact>` in
//! [`in_memory::InMemoryFactStore`].
//!
//! The trait is narrow enough that swapping in a library-backed
//! implementation later is a localized change. Provenance, authority,
//! and trace integration live above the trait (in dispatcher + bus
//! layers), not inside it.

pub mod in_memory;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::provenance::Provenance;
use crate::types::fact::{Fact, FactKey};
use crate::types::message::SubscribePattern;

pub use in_memory::InMemoryFactStore;

/// An immutable view of the fact space. Cheaply cloneable — shares
/// the underlying `HashMap` via `Arc`.
pub type FactSpaceSnapshot = Arc<HashMap<FactKey, Fact>>;

/// An event delivered to a subscriber.
#[derive(Clone, Debug)]
pub enum FactEvent {
    Asserted(Fact),
    Retracted {
        key: FactKey,
        provenance: Provenance,
    },
}

/// Handle returned by [`FactStore::subscribe`]. The caller drives the
/// receiver and drops it when done (dropping signals unsubscribe).
pub struct SubscriptionHandle {
    pub rx: mpsc::UnboundedReceiver<FactEvent>,
}

/// The fact-space interface.
///
/// Slice 001 uses the single [`InMemoryFactStore`] implementation. The
/// trait's existence is about forward-compatibility with the deferred
/// ECS-library decision (`research.md` §13) — swapping in a different
/// backing store later is a localized change.
pub trait FactStore {
    /// Assert a fact. If a fact with the same key already exists, this
    /// replaces it (and the new provenance becomes the current one).
    /// Subscribers matching the fact's family receive a
    /// [`FactEvent::Asserted`].
    fn assert(&mut self, fact: Fact);

    /// Retract a fact by key. If the fact existed, returns the prior
    /// value and broadcasts [`FactEvent::Retracted`] to matching
    /// subscribers. If no fact existed at the key, returns `None` and
    /// does not broadcast.
    fn retract(&mut self, key: &FactKey, provenance: Provenance) -> Option<Fact>;

    /// Query a fact by key.
    fn query(&self, key: &FactKey) -> Option<&Fact>;

    /// Subscribe to fact events matching the given pattern.
    fn subscribe(&mut self, pattern: SubscribePattern) -> SubscriptionHandle;

    /// Produce a cheap snapshot of all currently-asserted facts.
    fn snapshot(&self) -> FactSpaceSnapshot;
}
