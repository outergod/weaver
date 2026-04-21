//! Entity references — opaque, addressable identifiers per L1 §3.
//!
//! See `docs/01-system-model.md` §1 and `specs/001-hello-fact/data-model.md`.
//!
//! # CBOR tag
//!
//! On the bus, `EntityRef` is conceptually registered as CBOR tag 1000
//! (Weaver entity-ref) per `specs/001-hello-fact/contracts/bus-messages.md`.
//! Wire-level tag application at the codec layer is deferred for slice 001
//! (both ends of the bus are Weaver-controlled; transparent encoding
//! suffices). The registration prevents tag-number reuse in later slices.

use serde::{Deserialize, Serialize};

/// An opaque, stable reference to an entity. Entities have no intrinsic
/// type — interpretation arises from facts asserted about them
/// (L1 constitution §3–4).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityRef(u64);

impl EntityRef {
    /// Construct an `EntityRef` from a raw `u64`. Callers in slice 001 use
    /// this directly for synthetic buffer IDs (e.g., `EntityRef::new(1)`).
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The raw `u64` inside the ref.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for EntityRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityRef({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_json_transparent() {
        let e = EntityRef::new(42);
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(s, "42");
        assert_eq!(serde_json::from_str::<EntityRef>(&s).unwrap(), e);
    }

    #[test]
    fn ciborium_round_trip() {
        let e = EntityRef::new(42);
        let mut buf = Vec::new();
        ciborium::into_writer(&e, &mut buf).unwrap();
        let back: EntityRef = ciborium::from_reader(buf.as_slice()).unwrap();
        assert_eq!(back, e);
    }
}
