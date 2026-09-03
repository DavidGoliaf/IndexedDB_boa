//! Property-based tests for SCF-v1 encoding/decoding.

use boa_idb_core::clone::decode::decode_scf;
use boa_idb_core::clone::encode::encode_scf;
use boa_idb_core::clone::scvalue::*;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::limits::LimitConfig;
use indexmap::IndexMap;
use num_bigint::BigInt;
use proptest::prelude::*;

fn limits() -> LimitConfig {
    LimitConfig::default()
}

/// Generates arbitrary `ScValue` instances.
fn arb_scvalue() -> impl Strategy<Value = ScValue> {
    let leaf = prop_oneof![
        Just(ScValue::Undefined),
        Just(ScValue::Null),
        any::<bool>().prop_map(ScValue::Boolean),
        any::<f64>().prop_map(ScValue::Number),
        any::<i64>().prop_map(|i| ScValue::BigInt(BigInt::from(i))),
        Just(ScValue::String(Utf16String::new())),
        "[a-z]{0,10}".prop_map(|s| ScValue::String(s.into())),
        any::<f64>().prop_map(ScValue::Date),
        prop::collection::vec(any::<u8>(), 0..20).prop_map(|data| ScValue::ArrayBuffer {
            data,
            max_byte_length: None,
        }),
    ];
    leaf.prop_recursive(3, 5, 5, |inner| {
        prop_oneof![
            // Array
            prop::collection::vec(prop::option::of(inner.clone()), 0..5).prop_map(|elements| {
                ScValue::Array {
                    elements,
                    extra_props: Vec::new(),
                }
            }),
            // Object
            prop::collection::vec(
                (
                    "[a-z]{1,5}".prop_map(|s: String| Utf16String::from(s.as_str())),
                    inner.clone()
                ),
                0..5
            )
            .prop_map(|pairs| {
                let mut map = IndexMap::new();
                for (k, v) in pairs {
                    map.insert(k, v);
                }
                ScValue::Object(map)
            }),
            // Set
            prop::collection::vec(inner.clone(), 0..3).prop_map(ScValue::Set),
        ]
    })
}

proptest! {
    #[test]
    fn scf_roundtrip(value in arb_scvalue()) {
        let encoded = encode_scf(&value, &limits()).unwrap();
        let decoded = decode_scf(&encoded, &limits()).unwrap();
        prop_assert_eq!(&value, &decoded);
    }

    #[test]
    fn scf_garbage_never_panics(data in prop::collection::vec(any::<u8>(), 0..200)) {
        // The decoder must never panic, only return Err
        let _ = decode_scf(&data, &limits());
    }
}
