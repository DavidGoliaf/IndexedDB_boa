//! Unit tests for SCF-v1 encoding/decoding.

use boa_idb_core::clone::decode::decode_scf;
use boa_idb_core::clone::encode::{SCF_MAGIC, SCF_VERSION, encode_scf};
use boa_idb_core::clone::scvalue::*;
use boa_idb_core::clone::varint::*;
use boa_idb_core::error::ScError;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::limits::LimitConfig;
use indexmap::IndexMap;
use num_bigint::BigInt;

fn limits() -> LimitConfig {
    LimitConfig::default()
}

fn roundtrip(value: &ScValue) -> ScValue {
    let encoded = encode_scf(value, &limits()).unwrap();
    decode_scf(&encoded, &limits()).unwrap()
}

// ===== Varint tests =====

#[test]
fn varint_roundtrip_u64() {
    let values = [0u64, 1, 127, 128, 255, 256, 16383, 16384, u64::MAX];
    for &v in &values {
        let mut buf = Vec::new();
        encode_uvarint(v, &mut buf);
        let mut offset = 0;
        let decoded = decode_uvarint(&buf, &mut offset).unwrap();
        assert_eq!(decoded, v, "Failed for {v}");
        assert_eq!(offset, buf.len());
    }
}

#[test]
fn varint_roundtrip_i64() {
    let values = [0i64, 1, -1, 127, -128, i64::MAX, i64::MIN];
    for &v in &values {
        let mut buf = Vec::new();
        encode_ivarint(v, &mut buf);
        let mut offset = 0;
        let decoded = decode_ivarint(&buf, &mut offset).unwrap();
        assert_eq!(decoded, v, "Failed for {v}");
    }
}

// ===== SCF header tests =====

#[test]
fn scf_header_magic() {
    let encoded = encode_scf(&ScValue::Undefined, &limits()).unwrap();
    assert_eq!(&encoded[0..4], SCF_MAGIC);
    assert_eq!(encoded[4], SCF_VERSION);
}

// ===== Primitive round-trip tests =====

#[test]
fn scf_undefined() {
    assert_eq!(roundtrip(&ScValue::Undefined), ScValue::Undefined);
}

#[test]
fn scf_null() {
    assert_eq!(roundtrip(&ScValue::Null), ScValue::Null);
}

#[test]
fn scf_boolean() {
    assert_eq!(roundtrip(&ScValue::Boolean(true)), ScValue::Boolean(true));
    assert_eq!(roundtrip(&ScValue::Boolean(false)), ScValue::Boolean(false));
}

#[test]
fn scf_number_int32() {
    let val = ScValue::Number(42.0);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_number_negative_int32() {
    let val = ScValue::Number(-100.0);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_number_double() {
    let val = ScValue::Number(std::f64::consts::PI);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_bigint() {
    let val = ScValue::BigInt(BigInt::from(123_456_789_012_345_i64));
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_bigint_negative() {
    let val = ScValue::BigInt(BigInt::from(-42i64));
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_string_empty() {
    let val = ScValue::String(Utf16String::new());
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_string_ascii() {
    let val = ScValue::String("hello world".into());
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_string_with_surrogates() {
    let val = ScValue::String(Utf16String::from_slice(&[0xD800, 0x0041, 0xDFFF]));
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_date() {
    let val = ScValue::Date(1_234_567_890_000.0);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_regexp() {
    let val = ScValue::RegExp {
        pattern: "hello".into(),
        flags: RegExpFlags {
            global: true,
            ignore_case: true,
            ..Default::default()
        },
    };
    assert_eq!(roundtrip(&val), val);
}

// ===== Complex type tests =====

#[test]
fn scf_array() {
    let val = ScValue::Array {
        elements: vec![
            Some(ScValue::Number(1.0)),
            Some(ScValue::String("two".into())),
            None, // hole
            Some(ScValue::Boolean(true)),
        ],
        extra_props: vec![],
    };
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_object() {
    let mut map = IndexMap::new();
    map.insert(Utf16String::from("key1"), ScValue::Number(42.0));
    map.insert(Utf16String::from("key2"), ScValue::String("value".into()));
    let val = ScValue::Object(map);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_map() {
    let val = ScValue::Map(vec![
        (ScValue::String("a".into()), ScValue::Number(1.0)),
        (ScValue::String("b".into()), ScValue::Number(2.0)),
    ]);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_set() {
    let val = ScValue::Set(vec![
        ScValue::Number(1.0),
        ScValue::Number(2.0),
        ScValue::Number(3.0),
    ]);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_error() {
    let val = ScValue::Error(ScErrorObject {
        kind: ScErrorKind::TypeError,
        message: Some("test error".into()),
        cause: None,
        errors: None,
    });
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_arraybuffer() {
    let val = ScValue::ArrayBuffer {
        data: vec![0x01, 0x02, 0x03, 0x04],
        max_byte_length: None,
    };
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_boxed_boolean() {
    let val = ScValue::BoxedBoolean(true);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_boxed_number() {
    let val = ScValue::BoxedNumber(42.0);
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_boxed_string() {
    let val = ScValue::BoxedString("hello".into());
    assert_eq!(roundtrip(&val), val);
}

#[test]
fn scf_boxed_bigint() {
    let val = ScValue::BoxedBigInt(BigInt::from(999i64));
    assert_eq!(roundtrip(&val), val);
}

// ===== Error handling tests =====

#[test]
fn scf_invalid_magic() {
    let mut data = vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
    data.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // dummy crc
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::InvalidMagic)));
}

#[test]
fn scf_unsupported_version() {
    let mut data = b"IDB1".to_vec();
    data.push(2); // version 2
    data.push(0); // flags
    data.extend_from_slice(&[0, 0]); // reserved
    data.push(0x00); // body
    let checksum = boa_idb_core::clone::crc32c::crc32c(&data);
    data.extend_from_slice(&checksum.to_le_bytes());
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::UnsupportedVersion(2))));
}

#[test]
fn scf_checksum_mismatch() {
    let mut data = encode_scf(&ScValue::Null, &limits()).unwrap();
    // Corrupt the last byte (CRC)
    let last = data.len() - 1;
    data[last] ^= 0xFF;
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::ChecksumMismatch { .. })));
}

#[test]
fn scf_too_short() {
    let result = decode_scf(&[0, 1, 2, 3], &limits());
    assert!(result.is_err());
}

// ===== Nested structure test =====

#[test]
fn scf_deeply_nested() {
    let mut val = ScValue::Null;
    for _ in 0..10 {
        let mut map = IndexMap::new();
        map.insert(Utf16String::from("inner"), val);
        val = ScValue::Object(map);
    }
    assert_eq!(roundtrip(&val), val);
}

// ===== fix-01 memo-table and decoder-strictness regressions =====

/// Builds a complete SCF frame (header + body + CRC32C trailer).
fn frame_with_body(body: &[u8]) -> Vec<u8> {
    let mut data = b"IDB1".to_vec();
    data.push(1); // version
    data.push(0); // flags
    data.extend_from_slice(&[0, 0]); // reserved
    data.extend_from_slice(body);
    let checksum = boa_idb_core::clone::crc32c::crc32c(&data);
    data.extend_from_slice(&checksum.to_le_bytes());
    data
}

#[test]
fn scf_memo_ref_roundtrip() {
    // Memo layout: 0 = outer Array, 1 = ArrayBuffer.
    let buf = ScValue::ArrayBuffer {
        data: vec![1, 2, 3],
        max_byte_length: None,
    };
    let val = ScValue::Array {
        elements: vec![Some(buf.clone()), Some(ScValue::MemoRef(1))],
        extra_props: Vec::new(),
    };
    let decoded = roundtrip(&val);
    let ScValue::Array { elements, .. } = decoded else {
        panic!("expected Array");
    };
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0].as_ref(), Some(&buf));
    assert_eq!(elements[1].as_ref(), Some(&buf));
}

#[test]
fn scf_memo_ref_stays_aligned_across_typedarray() {
    // Memo layout: 0 = outer Array, 1 = buffer, 2 = TypedArray view.
    // Before the fix the decoder reserved no slot for TypedArray, so
    // MemoRef(2) resolved to the wrong entry.
    let buf = ScValue::ArrayBuffer {
        data: vec![10, 20, 30, 40],
        max_byte_length: None,
    };
    let view = ScValue::TypedArray {
        kind: ScTypedArrayKind::Uint8,
        byte_offset: 0,
        length: 4,
        buffer_memo_index: 1,
    };
    let val = ScValue::Array {
        elements: vec![
            Some(buf.clone()),
            Some(view.clone()),
            Some(ScValue::MemoRef(1)),
            Some(ScValue::MemoRef(2)),
        ],
        extra_props: Vec::new(),
    };
    let decoded = roundtrip(&val);
    let ScValue::Array { elements, .. } = decoded else {
        panic!("expected Array");
    };
    assert_eq!(elements.len(), 4);
    assert_eq!(elements[2].as_ref(), Some(&buf));
    assert_eq!(elements[3].as_ref(), Some(&view));
}

#[test]
fn scf_memo_ref_stays_aligned_across_boxed() {
    // Memo layout: 0 = Object, 1 = boxed number.
    let boxed = ScValue::BoxedNumber(42.0);
    let mut map = IndexMap::new();
    map.insert(Utf16String::from("b"), boxed.clone());
    map.insert(Utf16String::from("r"), ScValue::MemoRef(1));
    let decoded = roundtrip(&ScValue::Object(map));
    let ScValue::Object(out) = decoded else {
        panic!("expected Object");
    };
    assert_eq!(out.get(&Utf16String::from("r")), Some(&boxed));
}

#[test]
fn scf_forward_memo_ref_rejected() {
    // MemoRef(7) with an empty memo table.
    let data = frame_with_body(&[0x13, 0x07]);
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::InvalidMemoRef(7))));
}

#[test]
fn scf_bigint_bad_sign_rejected() {
    let mut encoded = encode_scf(&ScValue::BigInt(BigInt::from(5)), &limits()).unwrap();
    // Layout: header(8) + TAG_BIGINT(1) + sign(1) + len + bytes + crc(4).
    let body_end = encoded.len() - 4;
    encoded[9] = 0x02; // invalid sign byte
    let checksum = boa_idb_core::clone::crc32c::crc32c(&encoded[..body_end]);
    encoded[body_end..].copy_from_slice(&checksum.to_le_bytes());
    let result = decode_scf(&encoded, &limits());
    assert!(matches!(result, Err(ScError::CorruptedPayload(_))));
}

#[test]
fn scf_regexp_flags_overflow_rejected() {
    // RegExp with empty pattern, flags varint 0x200 (> u8 range).
    let body = [0x09, 0x00, 0x80, 0x04];
    let data = frame_with_body(&body);
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::CorruptedPayload(_))));
}

#[test]
fn scf_array_items_exceed_length_rejected() {
    // TAG_ARRAY, length = 0, items = 1: encoder invariant violated.
    let data = frame_with_body(&[0x0A, 0x00, 0x01]);
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::CorruptedPayload(_))));
}

#[test]
fn scf_array_index_out_of_bounds_rejected() {
    // TAG_ARRAY, length = 1, items = 1, index = 5 (no value needed: the
    // index itself is rejected before the value is read).
    let data = frame_with_body(&[0x0A, 0x01, 0x01, 0x05]);
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::CorruptedPayload(_))));
}

#[test]
fn scf_array_duplicate_index_rejected() {
    // TAG_ARRAY, length = 2, items = 2, index 0 twice (Null values).
    let data = frame_with_body(&[0x0A, 0x02, 0x02, 0x00, 0x01, 0x00, 0x01]);
    let result = decode_scf(&data, &limits());
    assert!(matches!(result, Err(ScError::CorruptedPayload(_))));
}

#[test]
fn scf_huge_string_length_never_allocates() {
    // TAG_STRING with a 10-byte varint length near u64::MAX and no body.
    // Must return Err (never panic, never attempt the allocation).
    let body = [
        0x06, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F,
    ];
    let data = frame_with_body(&body);
    let result = decode_scf(&data, &limits());
    assert!(result.is_err());
}
