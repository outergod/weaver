//! Facts — assertions about entities with provenance.
//!
//! See `specs/001-hello-fact/data-model.md` for the canonical shape.

use crate::provenance::Provenance;
use crate::types::entity_ref::EntityRef;
use serde::{Deserialize, Serialize};

/// Identifies a single fact in the fact space. A given `(entity, attribute)`
/// holds at most one value at a time — assertion replaces, retraction
/// removes.
///
/// The `attribute` is a slash-namespaced name; the namespace before the
/// first `/` identifies the fact family (e.g., `buffer/dirty` lives in
/// the `buffer` family).
///
/// # CBOR tag
///
/// The attribute string is conceptually registered as CBOR tag 1001
/// (Weaver keyword) per `specs/001-hello-fact/contracts/bus-messages.md`.
/// Wire-level tag application is deferred for slice 001 — see the note
/// on [`EntityRef`](crate::types::entity_ref::EntityRef) for rationale.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FactKey {
    pub entity: EntityRef,
    pub attribute: String,
}

impl FactKey {
    pub fn new(entity: EntityRef, attribute: impl Into<String>) -> Self {
        Self {
            entity,
            attribute: attribute.into(),
        }
    }

    /// The fact-family namespace (the prefix before the first `/`).
    /// An attribute without a `/` is treated as its own family name.
    pub fn family(&self) -> &str {
        self.attribute
            .split_once('/')
            .map(|(prefix, _)| prefix)
            .unwrap_or(self.attribute.as_str())
    }
}

impl std::fmt::Display for FactKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.entity, self.attribute)
    }
}

/// The value side of a fact. Slice 001 exercises `Bool`; slice 003 adds
/// `U64` for `buffer/byte-size`. Other variants land as fact families
/// grow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum FactValue {
    Bool(bool),
    String(String),
    Int(i64),
    /// Unsigned 64-bit integer. Slice 003 uses it for `buffer/byte-size`
    /// (see `specs/003-buffer-service/contracts/bus-messages.md`). Wire
    /// form: `{"type":"u64","value":<n>}`.
    U64(u64),
    Null,
}

/// A fact asserted on an entity with provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub key: FactKey,
    pub value: FactValue,
    pub provenance: Provenance,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::ActorIdentity;

    #[test]
    fn family_split() {
        let k = FactKey::new(EntityRef::new(1), "buffer/dirty");
        assert_eq!(k.family(), "buffer");
    }

    #[test]
    fn family_no_slash() {
        let k = FactKey::new(EntityRef::new(1), "standalone");
        assert_eq!(k.family(), "standalone");
    }

    #[test]
    fn fact_json_round_trip() {
        let f = Fact {
            key: FactKey::new(EntityRef::new(1), "buffer/dirty"),
            value: FactValue::Bool(true),
            provenance: Provenance::new(ActorIdentity::Core, 123, None).unwrap(),
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: Fact = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn fact_ciborium_round_trip() {
        let f = Fact {
            key: FactKey::new(EntityRef::new(7), "buffer/dirty"),
            value: FactValue::Bool(false),
            provenance: Provenance::new(ActorIdentity::Tui, 456, None).unwrap(),
        };
        let mut buf = Vec::new();
        ciborium::into_writer(&f, &mut buf).unwrap();
        let back: Fact = ciborium::from_reader(buf.as_slice()).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn factvalue_u64_json_round_trip() {
        for n in [0u64, 1, 42, 4096, u64::MAX / 2, u64::MAX] {
            let v = FactValue::U64(n);
            let s = serde_json::to_string(&v).unwrap();
            let back: FactValue = serde_json::from_str(&s).unwrap();
            assert_eq!(v, back);
            // Wire shape spot-check: adjacent-tag + snake_case variant name.
            assert!(
                s.contains("\"type\":\"u64\""),
                "wire form missing type=u64: {s}"
            );
        }
    }

    #[test]
    fn factvalue_u64_ciborium_round_trip() {
        for n in [0u64, 1, 42, u64::MAX] {
            let v = FactValue::U64(n);
            let mut buf = Vec::new();
            ciborium::into_writer(&v, &mut buf).unwrap();
            let back: FactValue = ciborium::from_reader(buf.as_slice()).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn factvalue_u64_distinguishable_from_int() {
        // `Int(i64)` and `U64(u64)` are distinct variants on the wire; a
        // round-trip MUST preserve which one was authored.
        let u = FactValue::U64(42);
        let i = FactValue::Int(42);
        let u_s = serde_json::to_string(&u).unwrap();
        let i_s = serde_json::to_string(&i).unwrap();
        assert_ne!(u_s, i_s);
        assert!(u_s.contains("u64"));
        assert!(i_s.contains("int"));
    }

    #[test]
    fn fact_with_u64_value_round_trip() {
        // Fact carrying a `U64` payload (per `buffer/byte-size`) must
        // round-trip through both wire formats.
        let f = Fact {
            key: FactKey::new(EntityRef::new(3), "buffer/byte-size"),
            value: FactValue::U64(18342),
            provenance: Provenance::new(ActorIdentity::Core, 789, None).unwrap(),
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: Fact = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
        let mut buf = Vec::new();
        ciborium::into_writer(&f, &mut buf).unwrap();
        let back: Fact = ciborium::from_reader(buf.as_slice()).unwrap();
        assert_eq!(f, back);
    }
}
