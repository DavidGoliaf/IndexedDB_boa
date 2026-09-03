//! Tests for key path parsing, extraction, and injection.

use boa_idb_core::clone::scvalue::ScValue;
use boa_idb_core::error::KeyPathError;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;
use indexmap::IndexMap;

// ===== Parsing tests =====

#[test]
fn parse_empty_string() {
    let kp = KeyPath::parse_single("").unwrap();
    assert_eq!(kp, KeyPath::Empty);
}

#[test]
fn parse_single_identifier() {
    let kp = KeyPath::parse_single("name").unwrap();
    assert_eq!(kp, KeyPath::Single("name".into()));
}

#[test]
fn parse_dotted_path() {
    let kp = KeyPath::parse_single("user.name.first").unwrap();
    assert_eq!(kp, KeyPath::Single("user.name.first".into()));
}

#[test]
fn parse_dollar_identifier() {
    let kp = KeyPath::parse_single("$id").unwrap();
    assert_eq!(kp, KeyPath::Single("$id".into()));
}

#[test]
fn parse_underscore_identifier() {
    let kp = KeyPath::parse_single("_private").unwrap();
    assert_eq!(kp, KeyPath::Single("_private".into()));
}

#[test]
fn parse_array_key_path() {
    let kp = KeyPath::parse_array(&["name", "age"]).unwrap();
    assert_eq!(kp, KeyPath::Array(vec!["name".into(), "age".into()]));
}

#[test]
fn parse_empty_array_rejected() {
    let result = KeyPath::parse_array(&[]);
    assert!(matches!(result, Err(KeyPathError::EmptyArray)));
}

// ===== Invalid key paths =====

#[test]
fn parse_invalid_start_digit() {
    let result = KeyPath::parse_single("123abc");
    assert!(matches!(result, Err(KeyPathError::InvalidSyntax(_))));
}

#[test]
fn parse_invalid_dot_dot() {
    let result = KeyPath::parse_single("a..b");
    assert!(matches!(result, Err(KeyPathError::InvalidSyntax(_))));
}

#[test]
fn parse_invalid_leading_dot() {
    let result = KeyPath::parse_single(".name");
    assert!(matches!(result, Err(KeyPathError::InvalidSyntax(_))));
}

#[test]
fn parse_invalid_trailing_dot() {
    let result = KeyPath::parse_single("name.");
    assert!(matches!(result, Err(KeyPathError::InvalidSyntax(_))));
}

#[test]
fn parse_invalid_space() {
    let result = KeyPath::parse_single("first name");
    assert!(matches!(result, Err(KeyPathError::InvalidSyntax(_))));
}

#[test]
fn parse_invalid_hyphen() {
    let result = KeyPath::parse_single("my-key");
    assert!(matches!(result, Err(KeyPathError::InvalidSyntax(_))));
}

// ===== Extraction tests =====

fn make_object(pairs: &[(&str, ScValue)]) -> ScValue {
    let mut map = IndexMap::new();
    for (k, v) in pairs {
        map.insert(Utf16String::from(*k), v.clone());
    }
    ScValue::Object(map)
}

#[test]
fn extract_empty_path() {
    let kp = KeyPath::Empty;
    let val = ScValue::Number(42.0);
    let result = kp.extract(&val).unwrap();
    assert_eq!(result, Some(Key::Number(42.0)));
}

#[test]
fn extract_single_path() {
    let kp = KeyPath::parse_single("name").unwrap();
    let val = make_object(&[("name", ScValue::String("Alice".into()))]);
    let result = kp.extract(&val).unwrap();
    assert_eq!(result, Some(Key::String("Alice".into())));
}

#[test]
fn extract_nested_path() {
    let kp = KeyPath::parse_single("user.name").unwrap();
    let val = make_object(&[(
        "user",
        make_object(&[("name", ScValue::String("Bob".into()))]),
    )]);
    let result = kp.extract(&val).unwrap();
    assert_eq!(result, Some(Key::String("Bob".into())));
}

#[test]
fn extract_missing_path_returns_none() {
    let kp = KeyPath::parse_single("missing").unwrap();
    let val = make_object(&[("name", ScValue::String("Alice".into()))]);
    let result = kp.extract(&val).unwrap();
    assert_eq!(result, None);
}

#[test]
fn extract_array_length() {
    let kp = KeyPath::parse_single("items.length").unwrap();
    let val = make_object(&[(
        "items",
        ScValue::Array {
            elements: vec![
                Some(ScValue::Number(1.0)),
                Some(ScValue::Number(2.0)),
                Some(ScValue::Number(3.0)),
            ],
            extra_props: vec![],
        },
    )]);
    let result = kp.extract(&val).unwrap();
    assert_eq!(result, Some(Key::Number(3.0)));
}

#[test]
fn extract_string_length() {
    let kp = KeyPath::parse_single("name.length").unwrap();
    let val = make_object(&[("name", ScValue::String("hello".into()))]);
    let result = kp.extract(&val).unwrap();
    assert_eq!(result, Some(Key::Number(5.0)));
}

#[test]
fn extract_array_key_path() {
    let kp = KeyPath::parse_array(&["first", "last"]).unwrap();
    let val = make_object(&[
        ("first", ScValue::String("John".into())),
        ("last", ScValue::String("Doe".into())),
    ]);
    let result = kp.extract(&val).unwrap();
    assert_eq!(
        result,
        Some(Key::Array(vec![
            Key::String("John".into()),
            Key::String("Doe".into()),
        ]))
    );
}

#[test]
fn extract_array_key_path_partial_failure() {
    let kp = KeyPath::parse_array(&["first", "missing"]).unwrap();
    let val = make_object(&[("first", ScValue::String("John".into()))]);
    let result = kp.extract(&val).unwrap();
    assert_eq!(result, None);
}

// ===== Injection tests =====

#[test]
fn inject_into_empty_object() {
    let kp = KeyPath::parse_single("id").unwrap();
    let mut val = ScValue::Object(IndexMap::new());
    kp.inject(&mut val, &Key::Number(42.0)).unwrap();
    match &val {
        ScValue::Object(map) => {
            assert_eq!(
                map.get(&Utf16String::from("id")),
                Some(&ScValue::Number(42.0))
            );
        }
        _ => panic!("Expected Object"),
    }
}

#[test]
fn inject_creates_intermediate_objects() {
    let kp = KeyPath::parse_single("a.b.c").unwrap();
    let mut val = ScValue::Object(IndexMap::new());
    kp.inject(&mut val, &Key::Number(1.0)).unwrap();
    // Should create: { a: { b: { c: 1.0 } } }
    match &val {
        ScValue::Object(map) => match map.get(&Utf16String::from("a")) {
            Some(ScValue::Object(inner)) => match inner.get(&Utf16String::from("b")) {
                Some(ScValue::Object(deep)) => {
                    assert_eq!(
                        deep.get(&Utf16String::from("c")),
                        Some(&ScValue::Number(1.0))
                    );
                }
                _ => panic!("Expected nested Object at b"),
            },
            _ => panic!("Expected nested Object at a"),
        },
        _ => panic!("Expected Object"),
    }
}

#[test]
fn inject_into_primitive_fails() {
    let kp = KeyPath::parse_single("a.b").unwrap();
    let mut val = make_object(&[("a", ScValue::Number(42.0))]);
    let result = kp.inject(&mut val, &Key::Number(1.0));
    assert!(matches!(
        result,
        Err(KeyPathError::CannotInjectIntoPrimitive)
    ));
}

#[test]
fn can_inject_into_object() {
    let kp = KeyPath::parse_single("a.b").unwrap();
    let val = ScValue::Object(IndexMap::new());
    assert!(kp.can_inject(&val));
}

#[test]
fn cannot_inject_into_primitive() {
    let kp = KeyPath::parse_single("a.b").unwrap();
    let val = make_object(&[("a", ScValue::Number(42.0))]);
    assert!(!kp.can_inject(&val));
}

#[test]
fn inject_empty_path_replaces_value() {
    let kp = KeyPath::Empty;
    let mut val = ScValue::Number(1.0);
    kp.inject(&mut val, &Key::String("replaced".into()))
        .unwrap();
    assert_eq!(val, ScValue::String("replaced".into()));
}
