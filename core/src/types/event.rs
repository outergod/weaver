//! Events — transient bus messages indicating something happened.
//!
//! Lossy delivery class per `docs/02-architecture.md` §3.1.

use crate::provenance::Provenance;
use crate::types::edit::TextEdit;
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
///
/// Slice 004 adds `BufferEdit { entity, version, edits }`: a versioned
/// batch of [`TextEdit`]s targeting an opened buffer. The
/// `weaver-buffers` service consumes the variant via its reader-loop,
/// validates against the buffer's current `buffer/version`, and
/// re-emits `buffer/byte-size` / `buffer/version` / `buffer/dirty`
/// facts on accept. Mismatched versions and validation failures are
/// silently dropped (lossy delivery class). See
/// `specs/004-buffer-edit/contracts/bus-messages.md` and
/// `specs/004-buffer-edit/data-model.md` for the wire/validation
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]
pub enum EventPayload {
    /// The buffer service opened a file and is claiming authority over
    /// its derived `buffer/*` facts. Idempotent at the fact level per
    /// FR-011a: receiving this for an already-owned entity is a no-op.
    BufferOpen { path: String },
    /// A versioned batch of text edits targeting an opened buffer.
    ///
    /// `entity` is the canonical buffer entity (matches `buffer/path`);
    /// `version` is the emitter's snapshot of `buffer/version` (the
    /// service accepts iff it matches the current value); `edits` is
    /// an atomic batch validated as a whole — any single-edit failure
    /// drops the entire batch with no observable mutation. Empty
    /// `edits: []` is a valid no-op.
    BufferEdit {
        entity: EntityRef,
        version: u64,
        edits: Vec<TextEdit>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ActorIdentity;
    use crate::types::edit::{Position, Range};
    use proptest::collection::vec as prop_vec;
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

    fn sample_buffer_edit(id: u64) -> Event {
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 0,
                },
            },
            new_text: "hello ".into(),
        };
        Event {
            id: EventId::new(id),
            name: "buffer/edit".into(),
            target: Some(EntityRef::new(42)),
            payload: EventPayload::BufferEdit {
                entity: EntityRef::new(42),
                version: 7,
                edits: vec![edit],
            },
            // ActorIdentity for the emitter envelope is set by the
            // CLI handler (slice-004 T013); for the wire-shape and
            // round-trip tests in this file, any non-User variant
            // suffices. The User-variant shape (unit vs struct) is
            // an open question deferred to T013.
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

    #[test]
    fn buffer_edit_wire_shape() {
        let e = sample_buffer_edit(11);
        let s = serde_json::to_string(&e).unwrap();
        // Adjacent-tagged variant per Amendment 5.
        assert!(
            s.contains("\"type\":\"buffer-edit\""),
            "expected adjacent tag `buffer-edit`: {s}"
        );
        // Payload carries entity + version + edits.
        assert!(s.contains("\"entity\":42"), "expected entity field: {s}");
        assert!(s.contains("\"version\":7"), "expected version field: {s}");
        // TextEdit serialises with kebab-case `new-text` on the wire.
        assert!(
            s.contains("\"new-text\":\"hello \""),
            "expected kebab-case new-text on TextEdit: {s}"
        );
        assert!(
            !s.contains("new_text"),
            "snake_case must not leak to the wire: {s}"
        );
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

    // ----- BufferEdit round-trip generators (T006) -----
    //
    // The validator's structural rules (R1..R6 + intra-batch overlap)
    // do NOT apply here: round-trip is a serialiser invariant, not a
    // semantic one. A 0x04 producer can dispatch a syntactically
    // well-formed but semantically invalid `BufferEdit` (e.g. an
    // out-of-bounds range); the wire codec MUST round-trip it
    // unchanged, and the service MUST drop it on validation. The
    // generators here therefore produce arbitrary bounded shapes
    // without enforcing semantic validity.

    fn gen_position() -> impl Strategy<Value = Position> {
        (0u32..1024, 0u32..1024).prop_map(|(line, character)| Position { line, character })
    }

    fn gen_range() -> impl Strategy<Value = Range> {
        (gen_position(), gen_position()).prop_map(|(a, b)| {
            // Order endpoints lexicographically so most generated
            // ranges are non-degenerate; the apply-edits validator
            // tests cover the swapped-endpoint path explicitly.
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            Range { start, end }
        })
    }

    fn gen_text_edit() -> impl Strategy<Value = TextEdit> {
        // ASCII-only `new_text` keeps proptest shrinking deterministic
        // and avoids accidental mid-codepoint Position generation
        // ambiguities — round-trip is encoding-agnostic anyway.
        (gen_range(), "[ -~]{0,32}").prop_map(|(range, new_text)| TextEdit { range, new_text })
    }

    fn gen_buffer_edit_payload() -> impl Strategy<Value = EventPayload> {
        (
            0u64..1_000_000,
            0u64..1_000_000,
            prop_vec(gen_text_edit(), 0..32),
        )
            .prop_map(|(entity_id, version, edits)| EventPayload::BufferEdit {
                entity: EntityRef::new(entity_id),
                version,
                edits,
            })
    }

    fn gen_buffer_edit_event() -> impl Strategy<Value = Event> {
        (0u64..1_000_000, gen_buffer_edit_payload(), 0u64..u64::MAX).prop_map(
            |(id, payload, ts)| Event {
                id: EventId::new(id),
                name: "buffer/edit".into(),
                target: Some(EntityRef::new(0xDEAD_BEEF)),
                payload,
                provenance: Provenance::new(ActorIdentity::Core, ts, None).unwrap(),
            },
        )
    }

    proptest! {
        #[test]
        fn buffer_edit_cbor_round_trip(e in gen_buffer_edit_event()) {
            let mut buf = Vec::new();
            ciborium::into_writer(&e, &mut buf).unwrap();
            let back: Event = ciborium::from_reader(buf.as_slice()).unwrap();
            prop_assert_eq!(e, back);
        }

        #[test]
        fn buffer_edit_json_round_trip(e in gen_buffer_edit_event()) {
            let s = serde_json::to_string(&e).unwrap();
            let back: Event = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(e, back);
        }
    }
}
