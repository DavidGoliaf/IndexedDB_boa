//! Unit tests for key primitives (P1-3 coverage drive).
//!
//! Pins `Utf16String` buffer operations, `Key::validate` rejection arms,
//! and the `Ord` implementation the backends rely on for range scans.

use boa_idb_core::error::KeyError;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use std::cmp::Ordering;

#[test]
fn utf16_buffer_ops() {
    let mut s = Utf16String::from("ab");
    assert_eq!(s.len(), 2);
    assert!(!s.is_empty());
    s.push(u16::from(b'c'));
    assert_eq!(s.len(), 3);
    s.extend_from_slice(&[u16::from(b'd'), 0x20AC]);
    assert_eq!(s.len(), 5);

    let empty = Utf16String::from("");
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    // Debug rendering never panics, even on lone surrogates.
    let mut lone = Utf16String::from("");
    lone.push(0xD800);
    assert!(!format!("{lone:?}").is_empty());
}

#[test]
fn key_validate_rejects_nan_and_deep_nesting() {
    let limits = LimitConfig::default();
    assert!(Key::Number(1.0).validate(&limits).is_ok());
    assert!(matches!(
        Key::Number(f64::NAN).validate(&limits),
        Err(KeyError::InvalidValue(_))
    ));
    assert!(matches!(
        Key::Date(f64::NAN).validate(&limits),
        Err(KeyError::InvalidValue(_))
    ));

    // Nesting deeper than max_key_depth fails.
    let mut deep = Key::Number(0.0);
    for _ in 0..=limits.max_key_depth + 1 {
        deep = Key::Array(vec![deep]);
    }
    assert!(matches!(
        deep.validate(&limits),
        Err(KeyError::MaxDepthExceeded(_))
    ));
}

#[test]
fn key_ordering_follows_type_order() {
    // Number < Date < String < Binary < Array (§2.4).
    let ordered = [
        Key::Number(1.0),
        Key::Date(0.0),
        Key::String(Utf16String::from("a")),
        Key::Binary(vec![0]),
        Key::Array(vec![Key::Number(0.0)]),
    ];
    for pair in ordered.windows(2) {
        assert_eq!(pair[0].cmp(&pair[1]), Ordering::Less, "{pair:?}");
        assert_eq!(pair[1].cmp(&pair[0]), Ordering::Greater, "{pair:?}");
        assert!(pair[0] < pair[1]);
        assert_eq!(pair[0].partial_cmp(&pair[1]), Some(Ordering::Less));
    }
    assert_eq!(Key::Number(1.0).cmp(&Key::Number(1.0)), Ordering::Equal);
    // NaN keys compare by bit pattern without panicking (validation, not
    // ordering, rejects them).
    assert_eq!(
        Key::Number(f64::NAN).cmp(&Key::Number(1.0)),
        Key::Number(f64::NAN).cmp(&Key::Number(1.0))
    );
}
