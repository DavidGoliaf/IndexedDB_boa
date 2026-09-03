//! Key path parsing, validation, extraction, and injection.
//!
//! Key paths follow the W3C IndexedDB §2.5 specification and ECMA-262 `IdentifierName` rules.

use crate::clone::scvalue::ScValue;
use crate::error::{KeyError, KeyPathError};
use crate::key::utf16::Utf16String;
use crate::key::value::Key;
use indexmap::IndexMap;

/// A parsed key path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPath {
    /// Empty key path (the value itself is the key).
    Empty,
    /// Single key path (dot-separated identifier chain).
    Single(Utf16String),
    /// Array of key paths.
    Array(Vec<Utf16String>),
}

impl KeyPath {
    /// Parses a key path string.
    ///
    /// - Empty string `""` → `KeyPath::Empty`
    /// - Non-empty string → `KeyPath::Single` (validated as dot-separated `IdentifierName`s)
    pub fn parse_single(s: &str) -> Result<Self, KeyPathError> {
        if s.is_empty() {
            return Ok(KeyPath::Empty);
        }
        validate_key_path_str(s)?;
        Ok(KeyPath::Single(s.into()))
    }

    /// Parses a key path from a string (for fuzz testing compatibility).
    pub fn parse(s: &str) -> Result<Self, KeyPathError> {
        Self::parse_single(s)
    }

    /// Parses an array key path.
    pub fn parse_array(paths: &[&str]) -> Result<Self, KeyPathError> {
        if paths.is_empty() {
            return Err(KeyPathError::EmptyArray);
        }
        let mut result = Vec::with_capacity(paths.len());
        for p in paths {
            validate_key_path_str(p)?;
            result.push((*p).into());
        }
        Ok(KeyPath::Array(result))
    }

    /// Extracts a key from a value according to this key path.
    ///
    /// Returns `Ok(None)` if any intermediate step resolves to `undefined`.
    pub fn extract(&self, value: &ScValue) -> Result<Option<Key>, KeyError> {
        match self {
            KeyPath::Empty => value.to_key(),
            KeyPath::Single(path) => extract_single_path(path, value),
            KeyPath::Array(paths) => {
                let mut result = Vec::with_capacity(paths.len());
                for p in paths {
                    match extract_single_path(p, value)? {
                        Some(k) => result.push(k),
                        None => return Ok(None),
                    }
                }
                Ok(Some(Key::Array(result)))
            }
        }
    }

    /// Checks whether a key can be injected into the given value without side effects.
    pub fn can_inject(&self, target: &ScValue) -> bool {
        match self {
            KeyPath::Empty => true,
            KeyPath::Single(path) => can_inject_single(path, target),
            KeyPath::Array(_) => false, // Array key paths are read-only
        }
    }

    /// Injects a key into a mutable value at this key path.
    ///
    /// Only works for `KeyPath::Single`. Creates intermediate objects as needed.
    pub fn inject(&self, target: &mut ScValue, key: &Key) -> Result<(), KeyPathError> {
        match self {
            KeyPath::Empty => {
                *target = ScValue::from_key(key);
                Ok(())
            }
            KeyPath::Single(path) => inject_single_path(path, target, key),
            KeyPath::Array(_) => Err(KeyPathError::InvalidSyntax(
                "Cannot inject into array key path".into(),
            )),
        }
    }
}

/// Validates that a string is a valid key path (dot-separated `IdentifierName`s).
fn validate_key_path_str(s: &str) -> Result<(), KeyPathError> {
    if s.is_empty() {
        return Ok(());
    }
    for part in s.split('.') {
        if part.is_empty() {
            return Err(KeyPathError::InvalidSyntax(
                "Empty identifier in key path".into(),
            ));
        }
        validate_identifier_name(part)?;
    }
    Ok(())
}

/// Validates that a string is a valid ECMA-262 `IdentifierName`.
///
/// Rules:
/// - First character: Unicode `ID_Start`, or `$`, or `_`.
/// - Subsequent characters: Unicode `ID_Continue`, or `$`, or `_`, or `U+200C` (ZWNJ), or `U+200D` (ZWJ).
/// - Escape sequences (`\uXXXX`) are NOT allowed.
fn validate_identifier_name(s: &str) -> Result<(), KeyPathError> {
    if s.is_empty() {
        return Err(KeyPathError::InvalidSyntax("Empty identifier".into()));
    }

    let mut chars = s.chars();
    let first = chars
        .next()
        .ok_or_else(|| KeyPathError::InvalidSyntax("Empty identifier".into()))?;

    if !is_id_start(first) {
        return Err(KeyPathError::InvalidSyntax(format!(
            "Invalid start character '{first}' in identifier"
        )));
    }

    for c in chars {
        if !is_id_continue(c) {
            return Err(KeyPathError::InvalidSyntax(format!(
                "Invalid character '{c}' in identifier"
            )));
        }
    }

    Ok(())
}

/// Checks if a character is valid as the first character of an identifier.
fn is_id_start(c: char) -> bool {
    c == '$' || c == '_' || unicode_id_start(c)
}

/// Checks if a character is valid as a continuation character of an identifier.
fn is_id_continue(c: char) -> bool {
    c == '$' || c == '_' || c == '\u{200C}' || c == '\u{200D}' || unicode_id_continue(c)
}

/// Simplified Unicode ID_Start check.
///
/// This covers the most common ranges. A full implementation would use
/// the `unicode-id` crate, but for IndexedDB key paths this is sufficient.
fn unicode_id_start(c: char) -> bool {
    matches!(c,
        'a'..='z' |
        'A'..='Z' |
        '\u{00C0}'..='\u{00D6}' |
        '\u{00D8}'..='\u{00F6}' |
        '\u{00F8}'..='\u{02FF}' |
        '\u{0370}'..='\u{037D}' |
        '\u{037F}'..='\u{1FFF}' |
        '\u{200E}'..='\u{200F}' |
        '\u{2070}'..='\u{218F}' |
        '\u{2C00}'..='\u{2FEF}' |
        '\u{3001}'..='\u{D7FF}' |
        '\u{F900}'..='\u{FDCF}' |
        '\u{FDF0}'..='\u{FFFD}' |
        '\u{10000}'..='\u{EFFFF}'
    )
}

/// Simplified Unicode ID_Continue check.
fn unicode_id_continue(c: char) -> bool {
    unicode_id_start(c)
        || matches!(c,
            '0'..='9' |
            '\u{0300}'..='\u{036F}' |
            '\u{1DC0}'..='\u{1DFF}' |
            '\u{20D0}'..='\u{20FF}' |
            '\u{FE20}'..='\u{FE2F}'
        )
}

/// Extracts a key from a value using a single dot-separated key path.
fn extract_single_path(path: &Utf16String, value: &ScValue) -> Result<Option<Key>, KeyError> {
    let path_str = path.to_string();
    let mut curr = value;

    for step in path_str.split('.') {
        match curr {
            ScValue::Object(map) => {
                let step_utf16: Utf16String = step.into();
                match map.get(&step_utf16) {
                    Some(v) => curr = v,
                    None => return Ok(None),
                }
            }
            ScValue::Array { elements, .. } => {
                if step == "length" {
                    let len = elements.len();
                    return Ok(Some(Key::Number(len as f64)));
                }
                return Ok(None);
            }
            ScValue::String(s) => {
                if step == "length" {
                    return Ok(Some(Key::Number(s.len() as f64)));
                }
                return Ok(None);
            }
            _ => return Ok(None),
        }
    }

    curr.to_key()
}

/// Checks if a key can be injected at the given path without side effects.
fn can_inject_single(path: &Utf16String, target: &ScValue) -> bool {
    let path_str = path.to_string();
    let parts: Vec<&str> = path_str.split('.').collect();

    if parts.is_empty() {
        return true;
    }

    let mut curr = target;
    for step in &parts[..parts.len() - 1] {
        match curr {
            ScValue::Object(map) => {
                let step_utf16: Utf16String = (*step).into();
                match map.get(&step_utf16) {
                    Some(v) => curr = v,
                    None => return true, // Will create intermediate object
                }
            }
            _ => return false, // Cannot traverse into primitives
        }
    }
    // Final node must be an Object to insert the key
    matches!(curr, ScValue::Object(_))
}

/// Injects a key into a value at the given single key path.
fn inject_single_path(
    path: &Utf16String,
    target: &mut ScValue,
    key: &Key,
) -> Result<(), KeyPathError> {
    let path_str = path.to_string();
    let parts: Vec<&str> = path_str.split('.').collect();

    if parts.is_empty() {
        return Ok(());
    }

    let mut curr = target;
    for step in &parts[..parts.len() - 1] {
        let step_utf16: Utf16String = (*step).into();
        match curr {
            ScValue::Object(map) => {
                if !map.contains_key(&step_utf16) {
                    map.insert(step_utf16.clone(), ScValue::Object(IndexMap::new()));
                }
                curr = map.get_mut(&step_utf16).ok_or_else(|| {
                    KeyPathError::InvalidSyntax("Failed to access intermediate object".into())
                })?;
            }
            _ => return Err(KeyPathError::CannotInjectIntoPrimitive),
        }
    }

    let last_step: Utf16String = parts.last().map(|s| (*s).into()).unwrap_or_default();
    match curr {
        ScValue::Object(map) => {
            map.insert(last_step, ScValue::from_key(key));
            Ok(())
        }
        _ => Err(KeyPathError::CannotInjectIntoPrimitive),
    }
}
