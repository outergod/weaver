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
    /// is for direct UUID-API needs (e.g., [`EventId::extract_prefix`]).
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    /// Mint a UUIDv8 EventId per slice-005 §28(a) re-derivation
    /// (`specs/005-buffer-save/research.md` §5).
    ///
    /// Bit layout (RFC 9562 UUIDv8, big-endian byte order):
    ///
    /// ```text
    ///   u128 bits 127..80 = custom_a (48 bits) — high 48 bits of
    ///                       the 58-bit `producer_prefix`.
    ///   u128 bits  79..76 = version nibble (= 0x8).
    ///   u128 bits  75..64 = custom_b (12 bits) — laid out as
    ///                       <prefix-low-10 bits><time-high-2 bits>.
    ///   u128 bits  63..62 = variant bits (= 0b10, RFC 4122 variant).
    ///   u128 bits  61..0  = custom_c (62 bits) — low 62 bits of
    ///                       `time_or_counter`.
    /// ```
    ///
    /// Total custom payload = 48 + 12 + 62 = 122 bits;
    /// `producer_prefix_58` (58 bits) + `time_or_counter` (64 bits) =
    /// 122 bits. The packing is invertible by [`EventId::extract_prefix`].
    ///
    /// Cross-producer collision is structurally impossible — distinct
    /// producers occupy disjoint 58-bit-prefix namespaces. Within a
    /// producer, the low-bits time/counter component plus producer-local
    /// monotonicity disambiguates.
    pub const fn mint_v8(producer_prefix_58: u64, time_or_counter: u64) -> Self {
        let prefix = producer_prefix_58 & ((1u64 << 58) - 1);
        let custom_a: u64 = prefix >> 10; // 48 bits
        let prefix_low_10: u64 = prefix & ((1u64 << 10) - 1);
        let time_high_2: u64 = (time_or_counter >> 62) & 0b11;
        let custom_b: u64 = (prefix_low_10 << 2) | time_high_2; // 12 bits
        let custom_c: u64 = time_or_counter & ((1u64 << 62) - 1); // 62 bits

        let mut value: u128 = 0;
        value |= (custom_a as u128) << 80;
        value |= 0x8_u128 << 76; // version nibble
        value |= (custom_b as u128 & 0xFFF) << 64;
        value |= 0b10_u128 << 62; // variant bits
        value |= custom_c as u128;

        Self(Uuid::from_u128(value))
    }

    /// Recover the 58-bit producer-prefix from a UUIDv8 EventId.
    ///
    /// Used by the slice-005 display layer (TUI + `weaver inspect`)
    /// to look up `prefix → friendly_name` bindings via passive cache
    /// (slice-005 tasks T-A3 + T-A4).
    ///
    /// Returns `0` for non-UUIDv8 inputs (e.g., [`EventId::nil`] or
    /// IDs minted as the slice-005 T-A1 transitional placeholder
    /// `Uuid::from_u128(now_ns() as u128)` whose version/variant bits
    /// are zero). Such IDs occupy the all-zero "unknown" prefix bucket
    /// in the display cache — a clean miss rather than a misattribution.
    pub const fn extract_prefix(&self) -> u64 {
        let value = self.0.as_u128();
        let custom_a: u64 = ((value >> 80) & ((1u128 << 48) - 1)) as u64;
        let custom_b: u64 = ((value >> 64) & ((1u128 << 12) - 1)) as u64;
        let prefix_low_10: u64 = (custom_b >> 2) & ((1u64 << 10) - 1);
        (custom_a << 10) | prefix_low_10
    }
}

/// Hash a [`Uuid`] to a 58-bit value for use as a UUIDv8 producer-prefix
/// per [`EventId::mint_v8`].
///
/// Stability contract: deterministic per-process (SipHash via
/// [`std::collections::hash_map::DefaultHasher`]); cross-process
/// stability is NOT guaranteed by `DefaultHasher`'s contract but is
/// sufficient for slice 005 because in-memory traces don't survive
/// listener restart anyway. A future slice that persists traces should
/// re-evaluate (likely switching to `xxhash-rust` for cross-version
/// stability).
///
/// Per `specs/005-buffer-save/research.md` §5 + §12.
pub fn hash_to_58(uuid: &Uuid) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    uuid.hash(&mut hasher);
    hasher.finish() & ((1u64 << 58) - 1)
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

    /// UUIDv8 mint produces a structurally-valid UUID: version nibble
    /// = 0x8, variant bits = 0b10. The `uuid` crate's `get_version_num`
    /// and `get_variant` accessors decode the same fields the codec
    /// strict-parsing path checks (SC-506).
    #[test]
    fn mint_v8_produces_valid_uuidv8() {
        let id = EventId::mint_v8(0x12345, 0xABCD_EF12_3456_7890);
        let uuid = id.as_uuid();
        assert_eq!(
            uuid.get_version_num(),
            8,
            "UUIDv8 version nibble must be 8: {uuid}"
        );
        assert_eq!(
            uuid.get_variant(),
            uuid::Variant::RFC4122,
            "UUIDv8 variant must be RFC 4122 (0b10): {uuid}"
        );
    }

    /// `extract_prefix` is the inverse of `mint_v8` for the
    /// producer-prefix component. Distinct prefixes produce distinct
    /// extractions; the time/counter low bits do not influence the
    /// extracted prefix.
    #[test]
    fn mint_v8_extract_prefix_round_trip() {
        for &prefix in &[0u64, 1, 0x12345, ((1u64 << 58) - 1), 0x2A2A_BEEF_CAFE_1234] {
            let masked_prefix = prefix & ((1u64 << 58) - 1);
            for &time in &[0u64, 1, 0xCAFE_BABE, u64::MAX] {
                let id = EventId::mint_v8(prefix, time);
                assert_eq!(
                    id.extract_prefix(),
                    masked_prefix,
                    "round-trip failed: prefix={prefix:#x} time={time:#x}"
                );
            }
        }
    }

    /// Distinct `(prefix, time)` pairs map to distinct UUIDs (as long
    /// as the time difference doesn't fall entirely in the dropped
    /// 2 high bits of the time component, which is structurally
    /// avoided by callers using `now_ns()`-scale values).
    #[test]
    fn mint_v8_distinct_inputs_distinct_outputs() {
        let a = EventId::mint_v8(0x1, 0x100);
        let b = EventId::mint_v8(0x2, 0x100);
        let c = EventId::mint_v8(0x1, 0x101);
        assert_ne!(a, b, "different prefixes must produce different UUIDs");
        assert_ne!(a, c, "different times must produce different UUIDs");
        assert_ne!(b, c);
    }

    /// `extract_prefix` returns 0 for `EventId::nil()` and for
    /// `for_testing` IDs whose u128 value has zero bits in the
    /// payload positions occupied by the producer-prefix.
    #[test]
    fn extract_prefix_returns_zero_for_non_uuidv8() {
        assert_eq!(EventId::nil().extract_prefix(), 0);
        // for_testing(N) wraps Uuid::from_u128(N); for small N the
        // high-bit positions where the producer-prefix lives are zero.
        assert_eq!(EventId::for_testing(42).extract_prefix(), 0);
    }

    /// `hash_to_58` is deterministic per-process and stays inside the
    /// 58-bit window required by `mint_v8`.
    #[test]
    fn hash_to_58_is_deterministic_and_bounded() {
        use uuid::Uuid;
        let u = Uuid::from_u128(0xDEAD_BEEF_CAFE_1234_5678_9ABC_DEF0_4242_u128);
        let h1 = hash_to_58(&u);
        let h2 = hash_to_58(&u);
        assert_eq!(h1, h2, "hash_to_58 must be deterministic per-process");
        assert!(
            h1 < (1u64 << 58),
            "hash_to_58 must fit in 58 bits: got {h1:#x}"
        );
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
