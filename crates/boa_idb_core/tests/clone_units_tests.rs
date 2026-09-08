//! Unit tests for SCF value helpers and codec edges (P1-3 coverage drive).
//!
//! Pins tag-enum roundtrips, `to_key`/`from_key` conversion rules (including
//! every rejection arm), malformed-input decode errors, and varint edges —
//! pure functions whose branches the integration suites never hit.

use boa_idb_core::clone::decode::decode_scf;
use boa_idb_core::clone::scvalue::{RegExpFlags, ScErrorKind, ScTypedArrayKind, ScValue};
use boa_idb_core::clone::varint::{decode_ivarint, decode_uvarint, encode_ivarint, encode_uvarint};
use boa_idb_core::error::KeyError;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use indexmap::IndexMap;

fn s(s: &str) -> Utf16String {
    Utf16String::from(s)
}

#[test]
fn error_kind_tag_roundtrip() {
    let kinds = [
        ScErrorKind::Error,
        ScErrorKind::EvalError,
        ScErrorKind::RangeError,
        ScErrorKind::ReferenceError,
        ScErrorKind::SyntaxError,
        ScErrorKind::TypeError,
        ScErrorKind::URIError,
        ScErrorKind::AggregateError,
    ];
    for (tag, kind) in kinds.iter().enumerate() {
        let tag = u8::try_from(tag).expect("fewer than 256 kinds");
        assert_eq!(ScErrorKind::from_u8(tag), Some(*kind));
    }
    assert_eq!(ScErrorKind::from_u8(8), None);
    assert_eq!(ScErrorKind::from_u8(255), None);
}

#[test]
fn typed_array_kind_tag_roundtrip() {
    for tag in 0..=11u8 {
        assert!(ScTypedArrayKind::from_u8(tag).is_some(), "tag {tag}");
    }
    assert_eq!(ScTypedArrayKind::from_u8(12), None);
    assert_eq!(ScTypedArrayKind::from_u8(255), None);
}

#[test]
fn regexp_flags_bitfield_roundtrip() {
    let all = RegExpFlags::from_bitfield(0xFF);
    assert!(all.has_indices && all.global && all.sticky);
    assert_eq!(all.to_bitfield(), 0xFF);

    let none = RegExpFlags::from_bitfield(0x00);
    assert_eq!(none.to_bitfield(), 0x00);

    // Single-bit flags survive the roundtrip individually.
    for bit in 0..8u8 {
        let flags = RegExpFlags::from_bitfield(1 << bit);
        assert_eq!(flags.to_bitfield(), 1 << bit, "bit {bit}");
    }
}

#[test]
fn to_key_accepts_scalars() {
    assert_eq!(
        ScValue::Number(2.5).to_key().unwrap(),
        Some(Key::Number(2.5))
    );
    assert_eq!(
        ScValue::Date(1_700_000_000_000.0).to_key().unwrap(),
        Some(Key::Date(1_700_000_000_000.0))
    );
    assert_eq!(
        ScValue::String(s("k")).to_key().unwrap(),
        Some(Key::String(s("k")))
    );
}

#[test]
fn to_key_rejects_nan_and_non_key_types() {
    assert!(matches!(
        ScValue::Number(f64::NAN).to_key(),
        Err(KeyError::InvalidValue(_))
    ));
    assert!(matches!(
        ScValue::Date(f64::NAN).to_key(),
        Err(KeyError::InvalidValue(_))
    ));
    // BigInt has its own rejection message.
    assert!(matches!(
        ScValue::BigInt(num_bigint::BigInt::from(7)).to_key(),
        Err(KeyError::InvalidType(_))
    ));
    for v in [
        ScValue::Boolean(true),
        ScValue::Undefined,
        ScValue::Null,
        ScValue::BoxedBoolean(false),
    ] {
        assert!(v.to_key().is_err(), "{v:?} must not be a key");
    }
    assert!(ScValue::Object(IndexMap::new()).to_key().is_err());
    assert!(ScValue::Map(Vec::new()).to_key().is_err());
    assert!(ScValue::Set(Vec::new()).to_key().is_err());
    assert!(ScValue::MemoRef(3).to_key().is_err());
}

#[test]
fn to_key_arrays_holes_and_nesting() {
    // Holes fail the whole conversion (return None, not Err).
    let with_hole = ScValue::Array {
        elements: vec![Some(ScValue::Number(1.0)), None],
        extra_props: Vec::new(),
    };
    assert_eq!(with_hole.to_key().unwrap(), None);

    // Invalid elements propagate the error.
    let bad_elem = ScValue::Array {
        elements: vec![Some(ScValue::Boolean(false))],
        extra_props: Vec::new(),
    };
    assert!(bad_elem.to_key().is_err());

    // Nested valid arrays convert recursively.
    let nested = ScValue::Array {
        elements: vec![
            Some(ScValue::Number(1.0)),
            Some(ScValue::Array {
                elements: vec![Some(ScValue::String(s("x")))],
                extra_props: Vec::new(),
            }),
        ],
        extra_props: Vec::new(),
    };
    assert_eq!(
        nested.to_key().unwrap(),
        Some(Key::Array(vec![
            Key::Number(1.0),
            Key::Array(vec![Key::String(s("x"))])
        ]))
    );
}

#[test]
fn from_key_covers_all_key_types() {
    assert_eq!(ScValue::from_key(&Key::Number(1.0)), ScValue::Number(1.0));
    assert_eq!(ScValue::from_key(&Key::Date(2.0)), ScValue::Date(2.0));
    assert_eq!(
        ScValue::from_key(&Key::String(s("a"))),
        ScValue::String(s("a"))
    );
    assert!(matches!(
        ScValue::from_key(&Key::Binary(vec![1, 2])),
        ScValue::ArrayBuffer { .. }
    ));
    assert!(matches!(
        ScValue::from_key(&Key::Array(vec![Key::Number(3.0)])),
        ScValue::Array { .. }
    ));
}

#[test]
fn decode_rejects_malformed_inputs() {
    let limits = LimitConfig::default();
    // Empty and short inputs.
    assert!(decode_scf(&[], &limits).is_err());
    assert!(decode_scf(&[0u8; 4], &limits).is_err());
    // Bad magic.
    assert!(
        decode_scf(
            &[0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0, 0, 0, 0, 0, 0, 0, 0],
            &limits
        )
        .is_err()
    );
    // Truncated header (magic ok, version missing).
    assert!(decode_scf(&[b'S', b'C', b'F', 0x01], &limits).is_err());
}

#[test]
fn varint_roundtrips_and_edges() {
    for v in [0u64, 1, 127, 128, 300, u64::MAX] {
        let mut out = Vec::new();
        encode_uvarint(v, &mut out);
        let mut off = 0;
        assert_eq!(decode_uvarint(&out, &mut off), Ok(v));
        assert_eq!(off, out.len());
    }
    for v in [0i64, -1, 127, -128, i64::MIN, i64::MAX] {
        let mut out = Vec::new();
        encode_ivarint(v, &mut out);
        let mut off = 0;
        assert_eq!(decode_ivarint(&out, &mut off), Ok(v));
        assert_eq!(off, out.len());
    }
    // Truncated input errors instead of panicking.
    let mut off = 0;
    assert!(decode_uvarint(&[0x80], &mut off).is_err());
    let mut off = 0;
    assert!(decode_uvarint(&[], &mut off).is_err());
}
