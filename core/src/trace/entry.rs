//! Trace entry types — append-only log of events, facts, and behavior firings.
//!
//! See `specs/001-hello-fact/data-model.md` and `docs/02-architecture.md` §10.

use crate::provenance::Provenance;
use crate::types::event::Event;
use crate::types::fact::{Fact, FactKey};
use crate::types::ids::{BehaviorId, EventId};
use crate::types::message::LifecycleSignal;
use serde::{Deserialize, Serialize};

/// Monotonic sequence number across the trace.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TraceSequence(u64);

impl TraceSequence {
    pub const ZERO: Self = Self(0);

    pub const fn new(n: u64) -> Self {
        Self(n)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl std::fmt::Display for TraceSequence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEntry {
    pub sequence: TraceSequence,
    pub timestamp_ns: u64,
    pub payload: TracePayload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TracePayload {
    Event {
        event: Event,
    },
    FactAsserted {
        fact: Fact,
    },
    FactRetracted {
        key: FactKey,
        provenance: Provenance,
    },
    BehaviorFired {
        behavior: BehaviorId,
        triggering_event: EventId,
        asserted: Vec<FactKey>,
        retracted: Vec<FactKey>,
        error: Option<String>,
    },
    Lifecycle(LifecycleSignal),
}
