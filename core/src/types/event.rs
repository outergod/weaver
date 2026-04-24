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
    /// Wire-stable event name (e.g., `"buffer/open"`).
    pub name: String,
    pub target: Option<EntityRef>,
    pub payload: EventPayload,
    pub provenance: Provenance,
}

/// Typed event payloads. The string `name` on [`Event`] is the
/// wire-stable identifier per L2 P7; this enum is the Rust face.
///
/// Slice 003 replaces the slice-001 `BufferEdited` / `BufferCleaned`
/// pair with a single `BufferOpen` event produced by the
/// `weaver-buffers` service's startup (FR-011). Dirty-state transitions
/// are now authoritative `buffer/dirty` facts authored by the service,
/// not events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]
pub enum EventPayload {
    /// The buffer service opened a file and is claiming authority over
    /// its derived `buffer/*` facts. Idempotent at the fact level per
    /// FR-011a: receiving this for an already-owned entity is a no-op.
    BufferOpen { path: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ActorIdentity;
    use proptest::prelude::*;

    fn sample_event(id: u64) -> Event {
        Event {
            id: EventId::new(id),
            name: "buffer/open".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(ActorIdentity::Tui, id.saturating_mul(1000), None).unwrap(),
        }
    }

    #[test]
    fn event_json_round_trip() {
        let e = sample_event(42);
        let s = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn buffer_open_wire_shape() {
        let e = sample_event(7);
        let s = serde_json::to_string(&e).unwrap();
        // Adjacent-tagged, kebab-case variant name per Amendment 5.
        assert!(
            s.contains("\"type\":\"buffer-open\""),
            "expected adjacent tag `buffer-open`: {s}"
        );
        assert!(
            s.contains("\"path\":\"/tmp/weaver-fixture\""),
            "expected path payload: {s}"
        );
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
