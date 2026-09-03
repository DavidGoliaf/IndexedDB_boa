//! Unit tests for key encoding/decoding with exact test vectors from the spec.

use boa_idb_core::key::compare::compare_keys;
use boa_idb_core::key::encode::{
    decode_f64_orderable, decode_key, encode_f64_orderable, encode_key,
};
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use std::cmp::Ordering;

fn limits() -> LimitConfig {
    LimitConfig::default()
}

fn encode(k: &Key) -> Vec<u8> {
    let mut out = Vec::new();
    encode_key(k, &mut out, &limits()).unwrap();
    out
}

// ===== Test vectors from §8.1 =====

#[test]
fn test_vector_number_1() {
    let key = Key::Number(1.0);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![0x10, 0xBF, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn test_vector_number_0() {
    let key = Key::Number(0.0);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![0x10, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn test_vector_number_neg_0() {
    // -0.0 must be normalized to +0.0
    let key = Key::Number(-0.0);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![0x10, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn test_vector_number_neg_1() {
    let key = Key::Number(-1.0);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![0x10, 0x40, 0x0F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    );
}

#[test]
fn test_vector_number_neg_infinity() {
    let key = Key::Number(f64::NEG_INFINITY);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![0x10, 0x00, 0x0F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
    );
}

#[test]
fn test_vector_number_infinity() {
    let key = Key::Number(f64::INFINITY);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![0x10, 0xFF, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn test_vector_date_0() {
    let key = Key::Date(0.0);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![0x20, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
}

#[test]
fn test_vector_string_empty() {
    let key = Key::String(Utf16String::new());
    let bytes = encode(&key);
    assert_eq!(bytes, vec![0x30, 0x00, 0x00]);
}

#[test]
fn test_vector_string_a() {
    let key = Key::String("a".into());
    let bytes = encode(&key);
    assert_eq!(bytes, vec![0x30, 0x61, 0x00, 0x00]);
}

#[test]
fn test_vector_string_null_char() {
    let key = Key::String(Utf16String::from_slice(&[0x0000]));
    let bytes = encode(&key);
    assert_eq!(bytes, vec![0x30, 0x00, 0xFF, 0x00, 0x00]);
}

#[test]
fn test_vector_string_unpaired_surrogate() {
    let key = Key::String(Utf16String::from_slice(&[0xD800]));
    let bytes = encode(&key);
    // 0xD800 → CESU-8: ED A0 80, escaped: ED A0 80 (no null bytes), terminator: 00 00
    assert_eq!(bytes, vec![0x30, 0xED, 0xA0, 0x80, 0x00, 0x00]);
}

#[test]
fn test_vector_binary() {
    let key = Key::Binary(vec![0x00, 0x01]);
    let bytes = encode(&key);
    assert_eq!(bytes, vec![0x40, 0x00, 0xFF, 0x01, 0x00, 0x00]);
}

#[test]
fn test_vector_array_empty() {
    let key = Key::Array(vec![]);
    let bytes = encode(&key);
    assert_eq!(bytes, vec![0x50, 0x00]);
}

#[test]
fn test_vector_array_one_number() {
    let key = Key::Array(vec![Key::Number(1.0)]);
    let bytes = encode(&key);
    assert_eq!(
        bytes,
        vec![
            0x50, 0x10, 0xBF, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
        ]
    );
}

#[test]
fn test_vector_array_nested_empty() {
    let key = Key::Array(vec![Key::Array(vec![])]);
    let bytes = encode(&key);
    assert_eq!(bytes, vec![0x50, 0x50, 0x00, 0x00]);
}

// ===== Round-trip tests =====

#[test]
fn roundtrip_number() {
    let values = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        42.5,
        -42.5,
    ];
    for &v in &values {
        let key = Key::Number(v);
        let encoded = encode(&key);
        let (decoded, consumed) = decode_key(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(compare_keys(&key, &decoded), Ordering::Equal);
    }
}

#[test]
fn roundtrip_date() {
    let key = Key::Date(1_234_567_890.0);
    let encoded = encode(&key);
    let (decoded, consumed) = decode_key(&encoded).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(compare_keys(&key, &decoded), Ordering::Equal);
}

#[test]
fn roundtrip_string() {
    let strings = ["", "a", "hello", "\u{0000}", "\u{0000}abc\u{0000}"];
    for s in &strings {
        let key = Key::String((*s).into());
        let encoded = encode(&key);
        let (decoded, consumed) = decode_key(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(compare_keys(&key, &decoded), Ordering::Equal);
    }
}

#[test]
fn roundtrip_string_unpaired_surrogates() {
    let key = Key::String(Utf16String::from_slice(&[0xD800, 0xDFFF, 0x0041]));
    let encoded = encode(&key);
    let (decoded, consumed) = decode_key(&encoded).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(compare_keys(&key, &decoded), Ordering::Equal);
}

#[test]
fn roundtrip_binary() {
    let key = Key::Binary(vec![0x00, 0x01, 0xFF, 0x00, 0x80]);
    let encoded = encode(&key);
    let (decoded, consumed) = decode_key(&encoded).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(compare_keys(&key, &decoded), Ordering::Equal);
}

#[test]
fn roundtrip_array() {
    let key = Key::Array(vec![
        Key::Number(1.0),
        Key::String("hello".into()),
        Key::Array(vec![Key::Date(0.0)]),
    ]);
    let encoded = encode(&key);
    let (decoded, consumed) = decode_key(&encoded).unwrap();
    assert_eq!(consumed, encoded.len());
    assert_eq!(compare_keys(&key, &decoded), Ordering::Equal);
}

// ===== Sorting invariant tests (§8.2) =====

#[test]
fn test_sorting_invariant() {
    let keys = vec![
        Key::Number(-1.0),
        Key::Number(0.0),
        Key::Number(1.0),
        Key::Date(0.0),
        Key::String("".into()),
        Key::String(Utf16String::from_slice(&[0x0000])),
        Key::String("a".into()),
        Key::Binary(vec![]),
        Key::Array(vec![]),
        Key::Array(vec![Key::Number(1.0)]),
    ];

    for i in 0..keys.len() {
        for j in i + 1..keys.len() {
            let encoded_i = encode(&keys[i]);
            let encoded_j = encode(&keys[j]);
            assert!(
                encoded_i < encoded_j,
                "Expected encode({:?}) < encode({:?})",
                keys[i],
                keys[j]
            );
            assert_eq!(
                compare_keys(&keys[i], &keys[j]),
                Ordering::Less,
                "Expected compare({:?}, {:?}) == Less",
                keys[i],
                keys[j]
            );
        }
    }
}

// ===== f64 order-preserving encoding tests =====

#[test]
fn test_f64_order_preserving() {
    let values = [
        f64::NEG_INFINITY,
        -1e308,
        -1.0,
        // -0.0 and 0.0 normalize to the same encoding
        0.0,
        1.0,
        1e308,
        f64::INFINITY,
    ];

    for i in 0..values.len() {
        for j in i + 1..values.len() {
            let enc_i = encode_f64_orderable(values[i]);
            let enc_j = encode_f64_orderable(values[j]);
            assert!(
                enc_i < enc_j,
                "Expected encode({}) < encode({})",
                values[i],
                values[j]
            );
        }
    }
}

#[test]
fn test_f64_neg_zero_equals_pos_zero() {
    let enc_neg = encode_f64_orderable(-0.0);
    let enc_pos = encode_f64_orderable(0.0);
    assert_eq!(enc_neg, enc_pos, "-0.0 and +0.0 must encode identically");
}

#[test]
fn test_f64_roundtrip() {
    let values = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        42.5,
        -42.5,
        f64::MIN,
        f64::MAX,
        f64::EPSILON,
        -f64::EPSILON,
    ];
    for &v in &values {
        let encoded = encode_f64_orderable(v);
        let decoded = decode_f64_orderable(encoded);
        // For -0.0, we normalize to 0.0
        let expected = if v == 0.0 { 0.0 } else { v };
        assert_eq!(decoded.to_bits(), expected.to_bits(), "Failed for {v}");
    }
}

// ===== Error cases =====

#[test]
fn test_nan_rejected() {
    let key = Key::Number(f64::NAN);
    let mut out = Vec::new();
    let result = encode_key(&key, &mut out, &limits());
    assert!(result.is_err());
}

#[test]
fn test_nan_date_rejected() {
    let key = Key::Date(f64::NAN);
    let mut out = Vec::new();
    let result = encode_key(&key, &mut out, &limits());
    assert!(result.is_err());
}

#[test]
fn test_empty_buffer_returns_eof() {
    let result = decode_key(&[]);
    assert!(result.is_err());
}

#[test]
fn test_unknown_tag_returns_error() {
    let result = decode_key(&[0x01]);
    assert!(result.is_err());
}

#[test]
fn test_key_too_large() {
    let limits = LimitConfig {
        max_key_len: 5,
        ..Default::default()
    };
    let key = Key::String("hello world".into());
    let mut out = Vec::new();
    let result = encode_key(&key, &mut out, &limits);
    assert!(result.is_err());
}

// ===== Comparison tests =====

#[test]
fn test_compare_same_type_numbers() {
    assert_eq!(
        compare_keys(&Key::Number(0.0), &Key::Number(-0.0)),
        Ordering::Equal
    );
    assert_eq!(
        compare_keys(&Key::Number(1.0), &Key::Number(2.0)),
        Ordering::Less
    );
    assert_eq!(
        compare_keys(&Key::Number(2.0), &Key::Number(1.0)),
        Ordering::Greater
    );
}

#[test]
fn test_compare_strings_code_unit_order() {
    // Code unit comparison: 0x0041 ('A') < 0x0061 ('a')
    let a = Key::String("A".into());
    let b = Key::String("a".into());
    assert_eq!(compare_keys(&a, &b), Ordering::Less);
}

#[test]
fn test_compare_arrays_lexicographic() {
    let a = Key::Array(vec![Key::Number(1.0)]);
    let b = Key::Array(vec![Key::Number(2.0)]);
    assert_eq!(compare_keys(&a, &b), Ordering::Less);

    let c = Key::Array(vec![Key::Number(1.0)]);
    let d = Key::Array(vec![Key::Number(1.0), Key::Number(0.0)]);
    assert_eq!(compare_keys(&c, &d), Ordering::Less); // shorter < longer
}
