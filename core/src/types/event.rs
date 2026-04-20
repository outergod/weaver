//! Events — transient bus messages indicating something happened.
//!
//! Lossy delivery class per `docs/02-architecture.md` §3.1.

use crate::provenance::Provenance;
use crate::types::entity_ref::EntityRef;
use crate::types::ids::EventId;
use serde::{Deserialize, Serialize};

/// An event published on the bus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    /// Wire-stable event name (e.g., `"buffer/edited"`).
    pub name: String,
    pub target: Option<EntityRef>,
    pub payload: EventPayload,
    pub provenance: Provenance,
}

/// Typed event payloads for slice 001. The string `name` is the
/// wire-stable identifier per L2 P7; this enum is the Rust face.
///
/// Future slices extend this enum as new event kinds become part of the
/// bus protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventPayload {
    BufferEdited,
    BufferCleaned,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::SourceId;
    use proptest::prelude::*;

    fn sample_event(id: u64) -> Event {
        Event {
            id: EventId::new(id),
            name: "buffer/edited".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferEdited,
            provenance: Provenance::new(SourceId::Tui, id.saturating_mul(1000), None).unwrap(),
        }
    }

    #[test]
    fn event_json_round_trip() {
        let e = sample_event(42);
        let s = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
    }

    proptest! {
        #[test]
        fn ciborium_round_trip(id in 0u64..1_000_000) {
            let e = sample_event(id);
            let mut buf = Vec::new();
            ciborium::into_writer(&e, &mut buf).unwrap();
            let back: Event = ciborium::from_reader(buf.as_slice()).unwrap();
            prop_assert_eq!(e, back);
        }
    }
}
