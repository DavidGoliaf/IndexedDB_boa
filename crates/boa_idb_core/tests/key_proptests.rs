//! Property-based tests for key encoding/decoding.

use boa_idb_core::key::compare::compare_keys;
use boa_idb_core::key::encode::{decode_key, encode_key};
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use proptest::prelude::*;

fn limits() -> LimitConfig {
    LimitConfig::default()
}

/// Generates arbitrary valid keys.
fn arb_key() -> impl Strategy<Value = Key> {
    let leaf = prop_oneof![
        // Special f64 values
        Just(Key::Number(0.0)),
        Just(Key::Number(-0.0)),
        Just(Key::Number(f64::INFINITY)),
        Just(Key::Number(f64::NEG_INFINITY)),
        // Regular numbers
        any::<f64>()
            .prop_filter("not NaN", |v| !v.is_nan())
            .prop_map(Key::Number),
        // Dates
        any::<f64>()
            .prop_filter("not NaN", |v| !v.is_nan())
            .prop_map(Key::Date),
        // Empty string
        Just(Key::String(Utf16String::new())),
        // ASCII strings
        "[a-z]{0,20}".prop_map(|s| Key::String(s.into())),
        // Strings with null chars
        prop::collection::vec(
            prop_oneof![
                0x0001u16..=0x007F,
                0x0080u16..=0x07FF,
                0x0800u16..=0xD7FF,
                0xE000u16..=0xFFFD,
            ],
            0..10
        )
        .prop_map(|v| Key::String(Utf16String::from_slice(&v))),
        // Strings with unpaired surrogates
        prop::collection::vec(0xD800u16..=0xDFFF, 0..5)
            .prop_map(|v| Key::String(Utf16String::from_slice(&v))),
        // Binary
        prop::collection::vec(any::<u8>(), 0..20).prop_map(Key::Binary),
        // Empty array
        Just(Key::Array(vec![])),
    ];
    leaf.prop_recursive(3, 5, 5, |inner| {
        prop::collection::vec(inner, 0..5).prop_map(Key::Array)
    })
}

proptest! {
    #[test]
    fn key_roundtrip(key in arb_key()) {
        let mut out = Vec::new();
        encode_key(&key, &mut out, &limits()).unwrap();
        let (decoded, consumed) = decode_key(&out).unwrap();
        prop_assert_eq!(consumed, out.len());
        prop_assert_eq!(compare_keys(&key, &decoded), std::cmp::Ordering::Equal);
    }

    #[test]
    fn key_order_preserving(a in arb_key(), b in arb_key()) {
        let cmp = compare_keys(&a, &b);
        let mut out_a = Vec::new();
        let mut out_b = Vec::new();
        encode_key(&a, &mut out_a, &limits()).unwrap();
        encode_key(&b, &mut out_b, &limits()).unwrap();
        let encoded_cmp = out_a.cmp(&out_b);
        prop_assert_eq!(cmp, encoded_cmp,
            "Order mismatch: cmp({:?}, {:?}) = {:?}, but encoded cmp = {:?}",
            a, b, cmp, encoded_cmp);
    }
}
