//! T068 — property test: every `BusMessage` variant that carries
//! `Provenance` on the wire has non-empty `source`.
//!
//! L2 P11 requires provenance on every published message. This
//! invariant is enforced at construction via [`Provenance::new`]
//! (which rejects `External("")`); this test demonstrates that no
//! wire-level encoding path sneaks an empty source through the
//! variants that carry provenance.
//!
//! Reference: `specs/001-hello-fact/tasks.md` T068.

use proptest::prelude::*;

use weaver_core::provenance::{Provenance, SourceId};
use weaver_core::types::entity_ref::EntityRef;
use weaver_core::types::event::{Event, EventPayload};
use weaver_core::types::fact::{Fact, FactKey, FactValue};
use weaver_core::types::ids::{BehaviorId, EventId};
use weaver_core::types::message::BusMessage;

/// Return the provenance field carried by a `BusMessage` variant, if
/// any. Variants without provenance (`Hello`, `SubscribeAck`,
/// `InspectRequest`, `Subscribe`, `Lifecycle`, `Error`,
/// `StatusRequest`, `StatusResponse`, `InspectResponse`) return
/// `None`.
fn provenance_of(msg: &BusMessage) -> Option<&Provenance> {
    match msg {
        BusMessage::Event(e) => Some(&e.provenance),
        BusMessage::FactAssert(f) => Some(&f.provenance),
        BusMessage::FactRetract { provenance, .. } => Some(provenance),
        _ => None,
    }
}

/// An `External` source built from a non-empty tag, guaranteed
/// round-trippable via CBOR / JSON.
fn arb_source() -> impl Strategy<Value = SourceId> {
    prop_oneof![
        Just(SourceId::Core),
        Just(SourceId::Tui),
        "[a-z]{1,20}".prop_map(SourceId::External),
        "[a-z]{1,10}/[a-z]{1,20}".prop_map(|id| SourceId::Behavior(BehaviorId::new(id))),
    ]
}

fn arb_provenance() -> impl Strategy<Value = Provenance> {
    (
        arb_source(),
        0u64..u64::MAX,
        proptest::option::of(0u64..u64::MAX),
    )
        .prop_map(|(source, ts, parent)| {
            Provenance::new(source, ts, parent.map(EventId::new))
                .expect("arb_provenance must produce a valid construction")
        })
}

fn arb_fact() -> impl Strategy<Value = Fact> {
    (
        0u64..100,
        "[a-z]{1,10}/[a-z]{1,10}",
        any::<bool>(),
        arb_provenance(),
    )
        .prop_map(|(e, attr, v, p)| Fact {
            key: FactKey::new(EntityRef::new(e), attr),
            value: FactValue::Bool(v),
            provenance: p,
        })
}

fn arb_event() -> impl Strategy<Value = Event> {
    (0u64..100, 0u64..100, any::<bool>(), arb_provenance()).prop_map(
        |(id, target, is_edit, provenance)| Event {
            id: EventId::new(id),
            name: if is_edit {
                "buffer/edited".into()
            } else {
                "buffer/cleaned".into()
            },
            target: Some(EntityRef::new(target)),
            payload: if is_edit {
                EventPayload::BufferEdited
            } else {
                EventPayload::BufferCleaned
            },
            provenance,
        },
    )
}

fn arb_provenanced_message() -> impl Strategy<Value = BusMessage> {
    prop_oneof![
        arb_event().prop_map(BusMessage::Event),
        arb_fact().prop_map(BusMessage::FactAssert),
        (arb_fact(), arb_provenance()).prop_map(|(f, p)| BusMessage::FactRetract {
            key: f.key,
            provenance: p,
        }),
    ]
}

fn source_is_non_empty(source: &SourceId) -> bool {
    match source {
        SourceId::Core | SourceId::Tui => true,
        SourceId::Behavior(id) => !id.as_str().is_empty(),
        SourceId::External(tag) => !tag.is_empty(),
    }
}

proptest! {
    /// After a CBOR round-trip, every provenance-carrying variant still
    /// has a non-empty `source`.
    #[test]
    fn cbor_round_trip_preserves_non_empty_source(msg in arb_provenanced_message()) {
        let mut buf = Vec::new();
        ciborium::into_writer(&msg, &mut buf).unwrap();
        let back: BusMessage = ciborium::from_reader(buf.as_slice()).unwrap();
        let source = provenance_of(&back)
            .map(|p| &p.source)
            .expect("generator only produces provenance-carrying variants");
        prop_assert!(source_is_non_empty(source));
    }

    /// After a JSON round-trip, same invariant.
    #[test]
    fn json_round_trip_preserves_non_empty_source(msg in arb_provenanced_message()) {
        let s = serde_json::to_string(&msg).unwrap();
        let back: BusMessage = serde_json::from_str(&s).unwrap();
        let source = provenance_of(&back)
            .map(|p| &p.source)
            .expect("generator only produces provenance-carrying variants");
        prop_assert!(source_is_non_empty(source));
    }
}
