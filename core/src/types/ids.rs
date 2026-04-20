//! Strongly-typed identifiers — distinct from each other and from
//! [`crate::types::entity_ref::EntityRef`].

use serde::{Deserialize, Serialize};

/// Event identifier. Monotonic per producer; unique for the lifetime of a
/// bus connection.
///
/// Authoritative bus messages (FactAssert, FactRetract, Lifecycle, Error)
/// carry sequence numbers per publisher — see
/// `docs/02-architecture.md` §3.1.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(u64);

impl EventId {
    pub const ZERO: Self = Self(0);

    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EventId({})", self.0)
    }
}

/// Human-readable behavior identifier, e.g., `"core/dirty-tracking"`.
///
/// Embedded Rust behaviors use Rust-qualified path conventions. Steel
/// behaviors (later slices) will use namespaced names such as
/// `"user:my-package:auto-save"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BehaviorId(String);

impl BehaviorId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BehaviorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_next() {
        assert_eq!(EventId::new(1).next(), EventId::new(2));
    }

    #[test]
    fn behavior_id_display() {
        let b = BehaviorId::new("core/dirty-tracking");
        assert_eq!(b.to_string(), "core/dirty-tracking");
    }

    #[test]
    fn event_id_ciborium_round_trip() {
        let id = EventId::new(123_456);
        let mut buf = Vec::new();
        ciborium::into_writer(&id, &mut buf).unwrap();
        let back: EventId = ciborium::from_reader(buf.as_slice()).unwrap();
        assert_eq!(back, id);
    }
}
