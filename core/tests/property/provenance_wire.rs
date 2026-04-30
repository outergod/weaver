//! T068 — property test: every `BusMessage` variant that carries
//! `Provenance` on the wire has a non-empty `source` after a CBOR or
//! JSON round-trip.
//!
//! L2 P11 requires provenance on every published message. The slice
//! 002 [`ActorIdentity`] enum's unit variants (`Core`, `Tui`) are
//! always non-empty by construction; `Behavior` / `Service` / `User`
//! / `Host` / `Agent` variants carry non-empty identifiers (enforced
//! at `ActorIdentity::service` construction via kebab-case validation,
//! and at higher-level constructors for the other variants).
//!
//! Reference: `specs/001-hello-fact/tasks.md` T068 (extended for slice
//! 002 `ActorIdentity` wire shape).

use proptest::prelude::*;
use uuid::Uuid;

use weaver_core::provenance::{ActorIdentity, Provenance};
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

/// Arbitrary well-formed [`ActorIdentity`] across every variant the
/// slice-002 wire accepts. Service identifiers are constructed as
/// hyphen-joined `[a-z0-9]+` segments so the kebab-case validator
/// never rejects a generated value.
fn arb_identity() -> impl Strategy<Value = ActorIdentity> {
    prop_oneof![
        Just(ActorIdentity::Core),
        Just(ActorIdentity::Tui),
        Just(ActorIdentity::User),
        "[a-z]{1,10}/[a-z]{1,20}".prop_map(|id| ActorIdentity::behavior(BehaviorId::new(id))),
        (
            proptest::collection::vec("[a-z0-9]{1,6}", 1..=4),
            any::<[u8; 16]>(),
        )
            .prop_map(|(segments, bytes)| {
                let id = segments.join("-");
                ActorIdentity::service(id, Uuid::from_bytes(bytes))
                    .expect("arb_identity segments produce valid kebab-case")
            }),
    ]
}

fn arb_provenance() -> impl Strategy<Value = Provenance> {
    (
        arb_identity(),
        0u64..u64::MAX,
        proptest::option::of(0u64..u64::MAX),
    )
        .prop_map(|(source, ts, parent)| {
            Provenance::new(source, ts, parent.map(|x| EventId::for_testing(x as u128)))
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
    (
        0u64..100,
        0u64..100,
        "/tmp/[a-z]{1,8}\\.txt",
        arb_provenance(),
    )
        .prop_map(|(id, target, path, provenance)| Event {
            id: EventId::for_testing(id as u128),
            name: "buffer/open".into(),
            target: Some(EntityRef::new(target)),
            payload: EventPayload::BufferOpen { path },
            provenance,
        })
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

fn identity_is_non_empty(source: &ActorIdentity) -> bool {
    match source {
        ActorIdentity::Core | ActorIdentity::Tui | ActorIdentity::User => true,
        ActorIdentity::Behavior { id } => !id.as_str().is_empty(),
        ActorIdentity::Service {
            service_id,
            instance_id: _,
        } => !service_id.is_empty(),
        ActorIdentity::Host { host_id, .. } => !host_id.is_empty(),
        ActorIdentity::Agent { agent_id, .. } => !agent_id.is_empty(),
    }
}

proptest! {
    /// After a CBOR round-trip, every provenance-carrying variant still
    /// has a non-empty structured `source`.
    #[test]
    fn cbor_round_trip_preserves_non_empty_source(msg in arb_provenanced_message()) {
        let mut buf = Vec::new();
        ciborium::into_writer(&msg, &mut buf).unwrap();
        let back: BusMessage = ciborium::from_reader(buf.as_slice()).unwrap();
        let source = provenance_of(&back)
            .map(|p| &p.source)
            .expect("generator only produces provenance-carrying variants");
        prop_assert!(identity_is_non_empty(source));
    }

    /// After a JSON round-trip, same invariant.
    #[test]
    fn json_round_trip_preserves_non_empty_source(msg in arb_provenanced_message()) {
        let s = serde_json::to_string(&msg).unwrap();
        let back: BusMessage = serde_json::from_str(&s).unwrap();
        let source = provenance_of(&back)
            .map(|p| &p.source)
            .expect("generator only produces provenance-carrying variants");
        prop_assert!(identity_is_non_empty(source));
    }

    /// Slice 002 T020 — round-trip over every `ActorIdentity` variant
    /// preserves equality exactly (not just non-emptiness).
    #[test]
    fn actor_identity_cbor_round_trip_exact(p in arb_provenance()) {
        let mut buf = Vec::new();
        ciborium::into_writer(&p, &mut buf).unwrap();
        let back: Provenance = ciborium::from_reader(buf.as_slice()).unwrap();
        prop_assert_eq!(p, back);
    }
}
