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

    /// Extracts the members of a multiEntry key path.
    ///
    /// Invalid members of an array are ignored by IndexedDB; they do not
    /// invalidate the other index entries produced by the record.
    pub fn extract_multi_entry(&self, value: &ScValue) -> Result<Vec<Key>, KeyError> {
        let raw = match self {
            KeyPath::Empty => Some(value),
            KeyPath::Single(path) => resolve_single_path(path, value),
            KeyPath::Array(_) => None,
        };
        let Some(raw) = raw else {
            return Ok(Vec::new());
        };
        match raw {
            ScValue::Array { elements, .. } => Ok(elements
                .iter()
                .filter_map(|element| element.as_ref()?.to_key().ok().flatten())
                .collect()),
            _ => Ok(raw.to_key()?.into_iter().collect()),
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
    let units: Vec<u16> = s.encode_utf16().collect();
    validate_key_path_units(&units)
}

/// Validates dot-separated `IdentifierName`s over raw UTF-16 code units.
fn validate_key_path_units(units: &[u16]) -> Result<(), KeyPathError> {
    for part in split_path_units(units) {
        if part.is_empty() {
            return Err(KeyPathError::InvalidSyntax(
                "Empty identifier in key path".into(),
            ));
        }
        validate_identifier_name(part)?;
    }
    Ok(())
}

/// Validates that a code-unit slice is a valid ECMA-262 `IdentifierName`.
///
/// Operates on raw UTF-16 code units so key paths containing unpaired
/// surrogates are validated without lossy conversion: lone surrogates are
/// never valid identifier characters and yield `InvalidSyntax`.
///
/// Rules:
/// - First character: Unicode `ID_Start`, or `$`, or `_`.
/// - Subsequent characters: Unicode `ID_Continue`, or `$`, or `_`, or `U+200C` (ZWNJ), or `U+200D` (ZWJ).
/// - Escape sequences (`\uXXXX`) are NOT allowed.
pub fn validate_identifier_name(units: &[u16]) -> Result<(), KeyPathError> {
    if units.is_empty() {
        return Err(KeyPathError::InvalidSyntax("Empty identifier".into()));
    }

    let mut pos = 0;
    let mut first = true;
    while pos < units.len() {
        let (decoded, consumed) = decode_unit(units, pos);
        let Some(c) = decoded else {
            return Err(KeyPathError::InvalidSyntax(
                "Unpaired surrogate in identifier".into(),
            ));
        };
        let valid = if first {
            is_id_start(c)
        } else {
            is_id_continue(c)
        };
        if !valid {
            return Err(KeyPathError::InvalidSyntax(format!(
                "Invalid character '{c}' in identifier"
            )));
        }
        pos += consumed;
        first = false;
    }

    Ok(())
}

/// Decodes one Unicode scalar value at `pos`.
///
/// Returns the character (or `None` for a lone surrogate) and the number of
/// code units consumed (1 or 2).
fn decode_unit(units: &[u16], pos: usize) -> (Option<char>, usize) {
    let u = units[pos];
    if (0xD800..0xDC00).contains(&u) {
        if pos + 1 < units.len() {
            let lo = units[pos + 1];
            if (0xDC00..0xE000).contains(&lo) {
                let high = u32::from(u - 0xD800);
                let low = u32::from(lo - 0xDC00);
                if let Some(c) = char::from_u32(0x1_0000 + (high << 10) + low) {
                    return (Some(c), 2);
                }
            }
        }
        return (None, 1);
    }
    if (0xDC00..0xE000).contains(&u) {
        return (None, 1);
    }
    (char::from_u32(u32::from(u)), 1)
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
/// the `unicode-ident` crate, but for IndexedDB key paths this is sufficient.
/// Note: `U+200E`/`U+200F` (bidi controls) are deliberately excluded — they
/// are not `ID_Start` in UAX #31.
fn unicode_id_start(c: char) -> bool {
    matches!(c,
        'a'..='z' |
        'A'..='Z' |
        '\u{00C0}'..='\u{00D6}' |
        '\u{00D8}'..='\u{00F6}' |
        '\u{00F8}'..='\u{02FF}' |
        '\u{0370}'..='\u{037D}' |
        '\u{037F}'..='\u{1FFF}' |
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

/// Splits a key path into steps on `.` (`U+002E`) over raw code units.
///
/// Unlike `str::split`, this preserves unpaired surrogates instead of
/// corrupting them through lossy UTF-8 conversion.
fn split_path_units(units: &[u16]) -> Vec<&[u16]> {
    let mut steps = Vec::new();
    let mut start = 0;
    for (i, &cu) in units.iter().enumerate() {
        if cu == 0x002E {
            steps.push(&units[start..i]);
            start = i + 1;
        }
    }
    steps.push(&units[start..]);
    steps
}

/// Checks whether a path step is the `length` pseudo-property.
fn is_length_step(step: &[u16]) -> bool {
    step == [0x006C, 0x0065, 0x006E, 0x0067, 0x0074, 0x0068] // "length"
}

/// Extracts a key from a value using a single dot-separated key path.
fn extract_single_path(path: &Utf16String, value: &ScValue) -> Result<Option<Key>, KeyError> {
    // An empty path means "the value itself is the key" (used for `''`
    // key paths and `['']` array components).
    if path.as_slice().is_empty() {
        return value.to_key();
    }
    let steps = split_path_units(path.as_slice());
    let mut curr = value;

    for (i, step) in steps.iter().enumerate() {
        let last = i + 1 == steps.len();
        match curr {
            ScValue::Object(map) => match map.get(&Utf16String::from_slice(step)) {
                Some(v) => curr = v,
                None => return Ok(None),
            },
            ScValue::Array { elements, .. } => {
                if is_length_step(step) {
                    if !last {
                        // `length` resolves to a number; traversing further
                        // into a number yields nothing.
                        return Ok(None);
                    }
                    return Ok(Some(Key::Number(elements.len() as f64)));
                }
                return Ok(None);
            }
            ScValue::String(s) => {
                if is_length_step(step) {
                    if !last {
                        return Ok(None);
                    }
                    return Ok(Some(Key::Number(s.len() as f64)));
                }
                return Ok(None);
            }
            _ => return Ok(None),
        }
    }

    curr.to_key()
}

/// Resolves a single path while preserving the final structured-clone value.
fn resolve_single_path<'a>(path: &Utf16String, value: &'a ScValue) -> Option<&'a ScValue> {
    if path.as_slice().is_empty() {
        return Some(value);
    }
    let steps = split_path_units(path.as_slice());
    let mut curr = value;
    for step in steps {
        match curr {
            ScValue::Object(map) => curr = map.get(&Utf16String::from_slice(step))?,
            _ => return None,
        }
    }
    Some(curr)
}

/// Checks if a key can be injected at the given path without side effects.
fn can_inject_single(path: &Utf16String, target: &ScValue) -> bool {
    let steps = split_path_units(path.as_slice());

    if steps.is_empty() {
        return true;
    }

    let mut curr = target;
    for step in &steps[..steps.len() - 1] {
        match curr {
            ScValue::Object(map) => {
                match map.get(&Utf16String::from_slice(step)) {
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
    let steps = split_path_units(path.as_slice());

    if steps.is_empty() {
        return Ok(());
    }

    let mut curr = target;
    for step in &steps[..steps.len() - 1] {
        let step_key = Utf16String::from_slice(step);
        match curr {
            ScValue::Object(map) => {
                if !map.contains_key(&step_key) {
                    map.insert(step_key.clone(), ScValue::Object(IndexMap::new()));
                }
                curr = map.get_mut(&step_key).ok_or_else(|| {
                    KeyPathError::InvalidSyntax("Failed to access intermediate object".into())
                })?;
            }
            _ => return Err(KeyPathError::CannotInjectIntoPrimitive),
        }
    }

    let last_step = Utf16String::from_slice(steps.last().map_or(&[], |s| *s));
    match curr {
        ScValue::Object(map) => {
            map.insert(last_step, ScValue::from_key(key));
            Ok(())
        }
        _ => Err(KeyPathError::CannotInjectIntoPrimitive),
    }
}
