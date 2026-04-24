//! T063 — property test: `FactValue::U64(n)` round-trips through JSON
//! and CBOR for arbitrary `n` across the full `u64` domain.
//!
//! The existing unit coverage in `core/src/types/fact.rs` exercises a
//! hand-picked sample (`0, 1, 42, 4096, u64::MAX / 2, u64::MAX`). Slice
//! 003 introduced the variant to carry `buffer/byte-size` — a value
//! that is legitimately expected to exceed `i64::MAX` on real files —
//! so the wire-round-trip invariant needs to hold universally, not
//! just at fixed points.
//!
//! Properties asserted:
//!
//! 1. `serde_json::from_str(serde_json::to_string(U64(n))) == U64(n)`
//!    for every `n: u64`.
//! 2. `ciborium::from_reader(ciborium::into_writer(U64(n))) == U64(n)`
//!    for every `n: u64`.
//! 3. On the wire, `U64(n)` and `Int(n as i64)` remain structurally
//!    distinct for any `n` in the shared `0..=i64::MAX` domain — the
//!    variant tag is preserved so a producer's intent survives the
//!    trip. Guards against a silent u64→int coercion regression.

use proptest::prelude::*;

use weaver_core::types::fact::FactValue;

proptest! {
    #[test]
    fn factvalue_u64_json_round_trip_exact(n in any::<u64>()) {
        let v = FactValue::U64(n);
        let s = serde_json::to_string(&v).expect("serialize FactValue::U64 to JSON");
        let back: FactValue = serde_json::from_str(&s).expect("deserialize FactValue::U64 from JSON");
        prop_assert_eq!(v, back);
        prop_assert!(s.contains("\"type\":\"u64\""), "wire form missing type=u64: {}", s);
    }

    #[test]
    fn factvalue_u64_cbor_round_trip_exact(n in any::<u64>()) {
        let v = FactValue::U64(n);
        let mut buf = Vec::new();
        ciborium::into_writer(&v, &mut buf).expect("serialize FactValue::U64 to CBOR");
        let back: FactValue = ciborium::from_reader(buf.as_slice()).expect("deserialize FactValue::U64 from CBOR");
        prop_assert_eq!(v, back);
    }

    #[test]
    fn factvalue_u64_and_int_remain_distinct_in_shared_domain(n in 0i64..=i64::MAX) {
        // Both variants are in-range for this `n`; the wire must not
        // conflate them. A bug that silently round-trips U64 as Int
        // would be a value-space regression — catch it here.
        let u = FactValue::U64(n as u64);
        let i = FactValue::Int(n);
        let u_json = serde_json::to_string(&u).expect("serialize U64");
        let i_json = serde_json::to_string(&i).expect("serialize Int");
        prop_assert_ne!(&u_json, &i_json);
        prop_assert!(u_json.contains("\"type\":\"u64\""));
        prop_assert!(i_json.contains("\"type\":\"int\""));
    }
}
