//! WebIDL type converters for IndexedDB arguments.
//!
//! Implements `[EnforceRange]`, `DOMString`, and sequence conversions
//! per the WebIDL specification and BOA_022_INCOMPATIBILITIES.md.

use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsNativeError, JsResult, JsValue};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;

use super::boa_compat::js_string_to_utf16;

/// Converts a `JsValue` to `u64` using `[EnforceRange]` rules.
///
/// Per WebIDL: NaN/Infinity → TypeError; truncates fractional part; range [0, 2^53−1].
pub fn to_unsigned_long_long_enforce_range(val: &JsValue, context: &mut Context) -> JsResult<u64> {
    let num = val.to_number(context)?;
    if num.is_nan() || num.is_infinite() {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is not a finite number")
            .into());
    }
    let integer = num.trunc();
    if integer < 0.0 || integer > u64::MAX as f64 {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is out of range for unsigned long long")
            .into());
    }
    Ok(integer as u64)
}

/// Converts a `JsValue` to `u32` using `[EnforceRange]` rules.
pub fn to_unsigned_long_enforce_range(val: &JsValue, context: &mut Context) -> JsResult<u32> {
    let num = val.to_number(context)?;
    if num.is_nan() || num.is_infinite() {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is not a finite number")
            .into());
    }
    let integer = num.trunc();
    if integer < 0.0 || integer > u32::MAX as f64 {
        return Err(JsNativeError::typ()
            .with_message("EnforceRange: value is out of range for unsigned long")
            .into());
    }
    Ok(integer as u32)
}

/// Converts a `JsValue` to `Utf16String` preserving surrogates.
///
/// Uses `JsString::to_vec()` to get raw UTF-16 code units without lossy conversion.
/// See BOA_022_INCOMPATIBILITIES.md §2.
pub fn to_dom_string(val: &JsValue, context: &mut Context) -> JsResult<Utf16String> {
    let js_str = val.to_string(context)?;
    Ok(js_string_to_utf16(&js_str))
}

/// Converts `(DOMString or sequence<DOMString>)` to a list of `Utf16String`.
///
/// If the value is an Array, each element is converted to a DOMString.
/// Otherwise, the value itself is treated as a single DOMString.
pub fn to_sequence_of_dom_strings(
    val: &JsValue,
    context: &mut Context,
) -> JsResult<Vec<Utf16String>> {
    if let Some(obj) = val.as_object() {
        if obj.is_array() {
            let arr = JsArray::from_object(obj.clone())?;
            let length = arr.length(context)? as u32;
            let mut result = Vec::with_capacity(length as usize);
            for i in 0..length {
                let elem = obj.get(i, context)?;
                result.push(to_dom_string(&elem, context)?);
            }
            return Ok(result);
        }
    }
    Ok(vec![to_dom_string(val, context)?])
}

/// Converts a `keyPath` argument (`null`, `DOMString`, or `sequence<DOMString>`) to `KeyPath`.
///
/// Per IndexedDB §5.1:
/// - `null` → `None`
/// - Empty string → `KeyPath::Empty`
/// - Non-empty string → `KeyPath::Single` (validated as identifier chain)
/// - Array → `KeyPath::Array` (each element validated)
pub fn to_key_path_argument(val: &JsValue, context: &mut Context) -> JsResult<Option<KeyPath>> {
    if val.is_null_or_undefined() {
        return Ok(None);
    }

    if let Some(obj) = val.as_object() {
        if obj.is_array() {
            let arr = JsArray::from_object(obj.clone())?;
            let length = arr.length(context)? as u32;
            if length == 0 {
                return Err(JsNativeError::syntax()
                    .with_message("KeyPath array cannot be empty")
                    .into());
            }
            let mut paths = Vec::with_capacity(length as usize);
            for i in 0..length {
                let elem = obj.get(i, context)?;
                let s = to_dom_string(&elem, context)?;
                // Validate as key path
                KeyPath::parse_single(&s.to_string())
                    .map_err(|e| JsNativeError::syntax().with_message(e.to_string()))?;
                paths.push(s);
            }
            return Ok(Some(KeyPath::Array(paths)));
        }
    }

    let s = to_dom_string(val, context)?;
    if s.is_empty() {
        return Ok(Some(KeyPath::Empty));
    }
    KeyPath::parse_single(&s.to_string())
        .map_err(|e| JsNativeError::syntax().with_message(e.to_string()))?;
    Ok(Some(KeyPath::Single(s)))
}

/// Converts a boolean, defaulting to `false` if undefined.
pub fn to_boolean_default_false(val: &JsValue) -> bool {
    !val.is_undefined() && val.to_boolean()
}
