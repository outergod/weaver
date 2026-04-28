//! Events — transient bus messages indicating something happened.
//!
//! Lossy delivery class per `docs/02-architecture.md` §3.1.

use crate::provenance::Provenance;
use crate::types::edit::TextEdit;
use crate::types::entity_ref::EntityRef;
use crate::types::ids::EventId;
use serde::{Deserialize, Serialize};

/// An event published on the bus.
///
/// Slice 005 splits the event lifecycle: producers serialise
/// [`EventOutbound`] (no `id`) and the bus listener allocates a
/// stamped [`EventId`] on accept, constructing the at-rest [`Event`]
/// via [`Event::from_outbound`]. Subscribers always observe `Event`.
/// See `specs/005-buffer-save/spec.md` FR-019..FR-024 and
/// `docs/07-open-questions.md` §28.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    /// Wire-stable event name (e.g., `"buffer/open"`).
    pub name: String,
    pub target: Option<EntityRef>,
    pub payload: EventPayload,
    pub provenance: Provenance,
}

impl Event {
    /// Promote an [`EventOutbound`] to the at-rest [`Event`] shape by
    /// stamping a listener-allocated [`EventId`]. This is the sole
    /// production path for `Event` under §28(a) — producers no
    /// longer mint EventIds.
    pub fn from_outbound(id: EventId, outbound: EventOutbound) -> Self {
        Self {
            id,
            name: outbound.name,
            target: outbound.target,
            payload: outbound.payload,
            provenance: outbound.provenance,
        }
    }
}

/// The wire-level shape of an event in flight from a producer to the
/// listener — [`Event`] minus `id`. The listener allocates a stamped
/// [`EventId`] on accept (per `specs/005-buffer-save/spec.md` FR-021)
/// via [`Event::from_outbound`]; producers MUST construct
/// `EventOutbound` and never mint `EventId` themselves.
///
/// `causal_parent` continues to live on `provenance.causal_parent` per
/// the slice-001 data model — Q1 (ID-stripped envelope) governs the
/// `id` field only; no `Provenance`-shape change rides slice 005.
///
/// `#[serde(deny_unknown_fields)]` is load-bearing: a producer that
/// erroneously serialises an `Event { id, .. }` shape on the inbound
/// channel hits a structured decode error (SC-506) rather than
/// silently dropping the `id` and producing a stamped event with
/// mismatched producer expectations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventOutbound {
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
///
/// Slice 005 adds `BufferSave { entity, version }`: a non-mutating
/// disk write-back request against an opened buffer. The
/// `weaver-buffers` service performs an atomic POSIX write
/// (tempfile + fsync + rename) gated by a pre-rename inode check; on
/// success it re-emits `buffer/dirty = false`. Save does not bump
/// `buffer/version`. See `specs/005-buffer-save/contracts/bus-messages.md`
/// and `specs/005-buffer-save/data-model.md`.
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
    /// A disk write-back request against an opened buffer.
    ///
    /// `entity` is the canonical buffer entity (matches `buffer/path`);
    /// `version` is the emitter's snapshot of `buffer/version` (the
    /// service accepts iff it matches the current value). Save is
    /// non-mutating w.r.t. content and version; on a dirty buffer the
    /// service performs an atomic disk write and re-emits
    /// `buffer/dirty = false` with `causal_parent = Some(event.id)`.
    /// On a clean buffer the service runs the no-op flow (idempotent
    /// `buffer/dirty = false` re-emission + `WEAVER-SAVE-007` info
    /// diagnostic; no disk I/O). See `specs/005-buffer-save/spec.md`
    /// FR-001..FR-007 and FR-017a.
    BufferSave { entity: EntityRef, version: u64 },
}

impl EventPayload {
    /// Adjacent-tag string used as the wire discriminator for this
    /// payload (same value the serde `#[serde(rename_all =
    /// "kebab-case")]` derive emits in the `"type"` field).
    ///
    /// Used by [`crate::types::message::EventSubscribePattern::matches`]
    /// to filter events at broadcast time without having to round-trip
    /// the payload through serde first.
    ///
    /// MUST stay in lockstep with the variant `rename_all` rule above —
    /// the regression test `event_payload_type_tag_matches_serde_discriminant`
    /// derives the discriminant via `serde_json::to_value` and pins both.
    pub fn type_tag(&self) -> &'static str {
        match self {
            Self::BufferOpen { .. } => "buffer-open",
            Self::BufferEdit { .. } => "buffer-edit",
            Self::BufferSave { .. } => "buffer-save",
        }
    }
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

    // ----- EventOutbound shape (slice 005, §28(a)) -----

    fn sample_outbound(seed: u64) -> EventOutbound {
        EventOutbound {
            name: "buffer/open".into(),
            target: Some(EntityRef::new(1)),
            payload: EventPayload::BufferOpen {
                path: "/tmp/weaver-fixture".into(),
            },
            provenance: Provenance::new(ActorIdentity::Tui, seed.saturating_mul(1000), None)
                .unwrap(),
        }
    }

    #[test]
    fn event_outbound_json_round_trip() {
        let o = sample_outbound(42);
        let s = serde_json::to_string(&o).unwrap();
        let back: EventOutbound = serde_json::from_str(&s).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn event_outbound_cbor_round_trip() {
        let o = sample_outbound(42);
        let mut buf = Vec::new();
        ciborium::into_writer(&o, &mut buf).unwrap();
        let back: EventOutbound = ciborium::from_reader(buf.as_slice()).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn event_from_outbound_preserves_fields() {
        let o = sample_outbound(42);
        let e = Event::from_outbound(EventId::new(7), o.clone());
        assert_eq!(e.id, EventId::new(7));
        assert_eq!(e.name, o.name);
        assert_eq!(e.target, o.target);
        assert_eq!(e.payload, o.payload);
        assert_eq!(e.provenance, o.provenance);
    }

    /// SC-506 regression: a producer that erroneously serialises an
    /// `Event { id, .. }` shape on the inbound channel must hit a
    /// structured decode error rather than silently dropping the
    /// `id` field. `#[serde(deny_unknown_fields)]` on `EventOutbound`
    /// is what enforces this — without it serde would accept the
    /// payload and produce a stamped event with an unintended id
    /// (the listener's allocation, not the producer's).
    #[test]
    fn event_outbound_rejects_id_field_on_deserialise() {
        let event_with_id = serde_json::json!({
            "id": 7,
            "name": "buffer/open",
            "target": 1,
            "payload": {"type": "buffer-open", "payload": {"path": "/tmp/x"}},
            "provenance": {
                "source": {"type": "tui"},
                "timestamp_ns": 1000,
                "causal_parent": null,
            },
        });
        let result: Result<EventOutbound, _> = serde_json::from_value(event_with_id);
        let err = result.expect_err("deny_unknown_fields must reject the `id` key");
        assert!(
            err.to_string().contains("id"),
            "decode error must name the unknown field: {err}"
        );
    }

    #[test]
    fn buffer_save_wire_shape() {
        // Slice 005: assert the adjacent-tagged JSON shape for the
        // new BufferSave EventPayload variant — `type` discriminator
        // is kebab-case `buffer-save`, payload carries `entity` and
        // `version`.
        let payload = EventPayload::BufferSave {
            entity: EntityRef::new(42),
            version: 7,
        };
        let s = serde_json::to_string(&payload).unwrap();
        assert!(
            s.contains("\"type\":\"buffer-save\""),
            "expected adjacent tag `buffer-save`: {s}"
        );
        assert!(s.contains("\"entity\":42"), "expected entity field: {s}");
        assert!(s.contains("\"version\":7"), "expected version field: {s}");
        let back: EventPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(payload, back);
    }

    /// Pin EventPayload::type_tag() against the serde-emitted "type"
    /// discriminator so the two cannot drift. EventSubscribePattern
    /// matching depends on this being byte-identical with the wire
    /// adjacent-tag string.
    #[test]
    fn event_payload_type_tag_matches_serde_discriminant() {
        for payload in [
            EventPayload::BufferOpen {
                path: "/tmp/x".into(),
            },
            EventPayload::BufferEdit {
                entity: EntityRef::new(1),
                version: 0,
                edits: vec![],
            },
            EventPayload::BufferSave {
                entity: EntityRef::new(1),
                version: 0,
            },
        ] {
            let v: serde_json::Value = serde_json::to_value(&payload).unwrap();
            let serde_tag = v
                .get("type")
                .and_then(|t| t.as_str())
                .expect("EventPayload always emits a `type` discriminator under adjacent tagging");
            assert_eq!(
                payload.type_tag(),
                serde_tag,
                "type_tag() must match the serde-emitted discriminator",
            );
        }
    }
}
