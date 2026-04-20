//! Provenance metadata required on every fact, event, and trace entry per
//! L2 constitution Principle 11.
//!
//! Every published bus message and every trace entry carries a
//! [`Provenance`] recording who produced it, when, and what caused it.
//! The `causal_parent` field enables `why?` walk-back through the trace
//! per `docs/02-architecture.md` §10.

use crate::types::ids::EventId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Provenance metadata attached to every fact, event, and trace entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: SourceId,
    /// Monotonic nanoseconds — by convention, since process start.
    pub timestamp_ns: u64,
    pub causal_parent: Option<EventId>,
}

/// The producer of a provenanced item.
///
/// `External` identifies out-of-process producers (future services,
/// agents). Future slices may split `External` into richer variants.
///
/// Wire shape (adjacent tagging with `"type"` discriminator and `"id"`
/// content field, kebab-case variant names per L2 Amendment 5):
/// `Core` → `{"type":"core"}`; `Behavior(id)` →
/// `{"type":"behavior","id":"core/dirty-tracking"}`;
/// `Tui` → `{"type":"tui"}`; `External(tag)` →
/// `{"type":"external","id":"agent-1"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "kebab-case")]
pub enum SourceId {
    Core,
    Behavior(crate::types::ids::BehaviorId),
    Tui,
    External(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProvenanceError {
    #[error("external source identifier must be non-empty (L2 P11)")]
    EmptyExternalSource,
}

impl Provenance {
    /// Construct a `Provenance`. Rejects empty `External("")` source per
    /// L2 P11 (provenance metadata must be attributable).
    pub fn new(
        source: SourceId,
        timestamp_ns: u64,
        causal_parent: Option<EventId>,
    ) -> Result<Self, ProvenanceError> {
        if let SourceId::External(s) = &source {
            if s.is_empty() {
                return Err(ProvenanceError::EmptyExternalSource);
            }
        }
        Ok(Self {
            source,
            timestamp_ns,
            causal_parent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ids::BehaviorId;
    use proptest::prelude::*;

    #[test]
    fn rejects_empty_external_source() {
        let r = Provenance::new(SourceId::External(String::new()), 0, None);
        assert_eq!(r.unwrap_err(), ProvenanceError::EmptyExternalSource);
    }

    #[test]
    fn accepts_valid_sources() {
        assert!(Provenance::new(SourceId::Core, 0, None).is_ok());
        assert!(Provenance::new(SourceId::Tui, 1, None).is_ok());
        assert!(
            Provenance::new(
                SourceId::Behavior(BehaviorId::new("core/dirty-tracking")),
                2,
                Some(EventId::new(7))
            )
            .is_ok()
        );
        assert!(Provenance::new(SourceId::External("agent-1".into()), 3, None).is_ok());
    }

    #[test]
    fn json_round_trip() {
        let p = Provenance::new(
            SourceId::Behavior(BehaviorId::new("core/dirty-tracking")),
            1000,
            Some(EventId::new(42)),
        )
        .unwrap();
        let s = serde_json::to_string(&p).unwrap();
        let back: Provenance = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    proptest! {
        #[test]
        fn ciborium_round_trip(ts in 0u64..u64::MAX, tag in "[a-z]{1,10}") {
            let p = Provenance::new(SourceId::External(tag), ts, None).unwrap();
            let mut buf = Vec::new();
            ciborium::into_writer(&p, &mut buf).unwrap();
            let back: Provenance = ciborium::from_reader(buf.as_slice()).unwrap();
            prop_assert_eq!(p, back);
        }

        #[test]
        fn non_empty_external_always_accepted(s in "[a-zA-Z0-9_-]+") {
            prop_assert!(Provenance::new(SourceId::External(s), 0, None).is_ok());
        }
    }
}
