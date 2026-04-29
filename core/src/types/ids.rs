//! Strongly-typed identifiers — distinct from each other and from
//! [`crate::types::entity_ref::EntityRef`].

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Event identifier.
///
/// Slice 005 §28(a) re-derivation (2026-04-29): `EventId` is now a
/// 16-byte [`Uuid`] minted as a UUIDv8 by the producer, with the
/// producer's hashed identity in the high 58 bits of the custom
/// payload (Service `instance_id` for Service producers; per-process
/// UUIDv4 for non-Service producers; both hashed via
/// [`std::collections::hash_map::DefaultHasher`] SipHash) and
/// nanoseconds (process-monotonic or wall-clock; producer's local
/// invariant) in the low 64 bits. The mint helper
/// [`EventId::mint_v8`] lands in slice-005 task T-A2; production
/// callers in slice-005 task T-A1 (this commit) use a transitional
/// [`Uuid::from_u128`] wrapper around the legacy `now_ns()`-based
/// scheme. The producer-mint-site UUIDv8 migration follows in
/// slice-005 tasks T009/T010/T011.
///
/// Cross-producer collision is structurally impossible — distinct
/// producers occupy disjoint 58-bit-prefix namespaces. Within a
/// producer, the low-bits nanosecond component plus producer-local
/// monotonicity disambiguates. See `specs/005-buffer-save/research.md`
/// §5 + §12 + `docs/00-constitution.md` audit.
///
/// [`EventId::nil`] (the all-zero UUID) is reserved for the
/// "no causal parent" sentinel; [`crate::bus::listener`] rejects
/// inbound events whose `id == nil()` and short-circuits inspect-side
/// walkbacks at the same value.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(Uuid);

impl EventId {
    /// Wrap an arbitrary [`Uuid`] as an `EventId`. Producer-mint sites
    /// in slice 005 T-A1 (this commit) wrap `Uuid::from_u128(now_ns()
    /// as u128)` here as a transitional placeholder; T-A2 introduces
    /// [`EventId::mint_v8`] for the UUIDv8 producer-prefix scheme.
    pub const fn new(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// The reserved "no causal parent" sentinel ([`Uuid::nil`] —
    /// all-zero bytes). Replaces slice-004's `EventId::ZERO` per
    /// FR-024. The listener rejects inbound events whose `id == nil()`
    /// (`validate_event_envelope`) and the inspect handler short-
    /// circuits walkbacks against this value.
    pub const fn nil() -> Self {
        Self(Uuid::nil())
    }

    /// Deterministic constructor for tests — wraps a `u128` into a
    /// [`Uuid`]. Use this in test fixtures where the EventId's value
    /// is irrelevant beyond uniqueness; production code paths use
    /// [`EventId::new`] (or [`EventId::mint_v8`] post-T-A2).
    pub const fn for_testing(value: u128) -> Self {
        Self(Uuid::from_u128(value))
    }

    /// Borrow the underlying [`Uuid`]. Most callers should let serde
    /// drive the wire shape via the transparent derive; this accessor
    /// is for direct UUID-API needs (e.g., `extract_prefix` in T-A2).
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Slice 005 T-A1: render the full UUID hex. Slice 005 T-A3/T-A4
        // adds passive-cache friendly_name rendering at the call sites
        // (TUI + `weaver inspect`) — `Display` itself stays uncached so
        // logging / tracing always have grep-able full UUIDs.
        write!(f, "EventId({})", self.0)
    }
}

/// Human-readable behavior identifier, e.g., `"core/<name>"` for
/// embedded behaviors or `"user:my-package:auto-save"` for Steel
/// behaviors in later slices. BehaviorId accepts any non-empty string;
/// `ActorIdentity::validate` enforces non-emptiness at the wire edge.
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
    fn event_id_for_testing_is_deterministic() {
        // The deterministic test constructor must always produce the
        // same UUID for the same input — used by fixture builders that
        // need stable equality across test runs.
        assert_eq!(EventId::for_testing(42), EventId::for_testing(42));
        assert_ne!(EventId::for_testing(1), EventId::for_testing(2));
    }

    #[test]
    fn event_id_nil_is_uuid_nil() {
        assert_eq!(EventId::nil(), EventId::new(Uuid::nil()));
    }

    #[test]
    fn behavior_id_display() {
        let b = BehaviorId::new("core/dirty-tracking");
        assert_eq!(b.to_string(), "core/dirty-tracking");
    }

    #[test]
    fn event_id_ciborium_round_trip() {
        let id = EventId::for_testing(123_456);
        let mut buf = Vec::new();
        ciborium::into_writer(&id, &mut buf).unwrap();
        let back: EventId = ciborium::from_reader(buf.as_slice()).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn event_id_json_round_trip_emits_hex_string() {
        // Wire shape commitment: JSON emits the hex-with-hyphens UUID
        // form (the `uuid` crate's default `Serialize` for human-
        // readable formats). This is the shape `weaver inspect
        // --output=json` exposes for `event.id` and the
        // `fact_inspection.source_event` field.
        let id = EventId::for_testing(0x01863f4e_9c2a_8000_8421_c5d2e4f6a7b8_u128);
        let s = serde_json::to_string(&id).unwrap();
        assert!(
            s.starts_with('\"') && s.ends_with('\"'),
            "EventId JSON must be a string: {s}"
        );
        assert!(
            s.contains("01863f4e-9c2a-8000-8421-c5d2e4f6a7b8"),
            "EventId JSON must contain hex-with-hyphens UUID: {s}"
        );
        let back: EventId = serde_json::from_str(&s).unwrap();
        assert_eq!(back, id);
    }
}
