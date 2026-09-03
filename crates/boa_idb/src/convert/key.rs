//! Converters between `JsValue` and `Key` (§7.3, §7.4).
//!
//! Implements the IndexedDB key construction algorithm with support for
//! Number, Date, String, Array, ArrayBuffer, TypedArray, and DataView.
//! See BOA_022_INCOMPATIBILITIES.md §1, §2, §7, §8.

use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsDate, JsTypedArray};
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::error::KeyError;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;

use super::boa_compat::{JsObjectExt, create_array_buffer, js_string_to_utf16};

/// Maximum array nesting depth for keys.
const MAX_KEY_DEPTH: usize = 32;

/// Converts a `JsValue` to a `Key` (§7.3).
///
/// Supports: Number, Date, String, Array, ArrayBuffer, TypedArray, DataView.
/// Returns `KeyError::InvalidType` for unsupported types (null, undefined, Object, Function, Symbol).
pub fn value_to_key(val: &JsValue, context: &mut Context) -> Result<Key, KeyError> {
    value_to_key_depth(val, context, 0)
}

fn value_to_key_depth(val: &JsValue, context: &mut Context, depth: usize) -> Result<Key, KeyError> {
    if depth > MAX_KEY_DEPTH {
        return Err(KeyError::MaxDepthExceeded(MAX_KEY_DEPTH));
    }

    // Number — используем as_number() без аллокации (BOA_022 §1)
    if let Some(n) = val.as_number() {
        if n.is_nan() {
            return Err(KeyError::InvalidValue("NaN is not a valid key".into()));
        }
        return Ok(Key::Number(n));
    }

    // String — используем as_string() + to_vec() для сохранения суррогатов (BOA_022 §2)
    if let Some(s) = val.as_string() {
        return Ok(Key::String(js_string_to_utf16(&s)));
    }

    // Object (Date, Array, ArrayBuffer, TypedArray, DataView)
    if let Some(obj) = val.as_object() {
        return value_object_to_key(&obj, context, depth);
    }

    // BigInt — not a valid key type
    if val.is_bigint() {
        return Err(KeyError::InvalidType(
            "BigInt cannot be used as a key".into(),
        ));
    }

    // null, undefined, Symbol, Function
    Err(KeyError::InvalidType(format!(
        "Cannot convert {:?} to key",
        val.type_of()
    )))
}

/// Converts an object JsValue to a Key.
fn value_object_to_key(
    obj: &JsObject,
    context: &mut Context,
    depth: usize,
) -> Result<Key, KeyError> {
    // Check for Date — используем JsDate::from_object (BOA_022 §9)
    if let Ok(date) = JsDate::from_object(obj.clone()) {
        if let Ok(time_val) = date.get_time(context) {
            if let Some(d) = time_val.as_number() {
                if d.is_nan() {
                    return Err(KeyError::InvalidValue("Date value is NaN".into()));
                }
                return Ok(Key::Date(d));
            }
        }
    }

    // Check for Array — используем JsArray::from_object (BOA_022 §6)
    if obj.is_array() {
        return array_to_key(obj, context, depth);
    }

    // Check for TypedArray — must check before ArrayBuffer since TypedArray has a buffer
    if let Ok(typed) = JsTypedArray::from_object(obj.clone()) {
        return typed_array_to_key(&typed, context);
    }

    // Check for ArrayBuffer — используем JsArrayBuffer::from_object (BOA_022 §7)
    if let Ok(buf) = JsArrayBuffer::from_object(obj.clone()) {
        if let Some(data) = buf.data() {
            return Ok(Key::Binary(data.to_vec()));
        }
        return Err(KeyError::InvalidValue("ArrayBuffer is detached".into()));
    }

    // Check for DataView — it has buffer, byteOffset, byteLength properties
    if let Some(bytes) = extract_data_view_bytes(obj, context) {
        return Ok(Key::Binary(bytes));
    }

    Err(KeyError::InvalidType("Cannot convert object to key".into()))
}

/// Converts an Array to a Key with cycle detection.
fn array_to_key(obj: &JsObject, context: &mut Context, depth: usize) -> Result<Key, KeyError> {
    let length =
        obj.array_length(context)
            .map_err(|_| KeyError::InvalidType("Cannot get array length".into()))? as u32;

    let mut keys = Vec::with_capacity(length as usize);
    for i in 0..length {
        let elem = obj
            .get(i, context)
            .map_err(|_| KeyError::InvalidType("Cannot get array element".into()))?;

        // Check for array holes (explicit undefined vs missing property)
        if elem.is_undefined() && !obj.has_property(i, context).unwrap_or(false) {
            return Err(KeyError::InvalidValue(
                "Array holes are not allowed in keys".into(),
            ));
        }

        let key = value_to_key_depth(&elem, context, depth + 1)?;
        keys.push(key);
    }
    Ok(Key::Array(keys))
}

/// Extracts bytes from a TypedArray.
fn typed_array_to_key(typed: &JsTypedArray, context: &mut Context) -> Result<Key, KeyError> {
    let byte_offset = typed
        .byte_offset(context)
        .map_err(|_| KeyError::InvalidType("Cannot get TypedArray byteOffset".into()))?;
    let byte_length = typed
        .byte_length(context)
        .map_err(|_| KeyError::InvalidType("Cannot get TypedArray byteLength".into()))?;

    let buffer_val = typed
        .buffer(context)
        .map_err(|_| KeyError::InvalidType("Cannot get TypedArray buffer".into()))?;

    let buf_obj = buffer_val
        .as_object()
        .ok_or_else(|| KeyError::InvalidType("TypedArray buffer is not an object".into()))?;

    let buf = JsArrayBuffer::from_object(buf_obj.clone())
        .map_err(|_| KeyError::InvalidType("Cannot access TypedArray buffer".into()))?;

    if let Some(data) = buf.data() {
        let end = byte_offset + byte_length;
        if end <= data.len() {
            return Ok(Key::Binary(data[byte_offset..end].to_vec()));
        }
    }

    Err(KeyError::InvalidValue(
        "TypedArray buffer is detached or out of bounds".into(),
    ))
}

/// Attempts to extract bytes from a DataView object.
fn extract_data_view_bytes(obj: &JsObject, context: &mut Context) -> Option<Vec<u8>> {
    // DataView has 'buffer', 'byteOffset', 'byteLength' properties
    // We check if it has a 'buffer' property that is an ArrayBuffer
    let buffer_val = obj.get(js_string!("buffer"), context).ok()?;
    let buf_obj = buffer_val.as_object()?;
    let buf = JsArrayBuffer::from_object(buf_obj.clone()).ok()?;

    let byte_offset = obj
        .get(js_string!("byteOffset"), context)
        .ok()?
        .as_number()? as usize;
    let byte_length = obj
        .get(js_string!("byteLength"), context)
        .ok()?
        .as_number()? as usize;

    let data = buf.data()?;
    let end = byte_offset + byte_length;
    if end <= data.len() {
        Some(data[byte_offset..end].to_vec())
    } else {
        None
    }
}

/// Converts a `Key` to a `JsValue` (§7.4).
pub fn key_to_value(key: &Key, context: &mut Context) -> JsResult<JsValue> {
    match key {
        Key::Number(n) => Ok(JsValue::from(*n)),
        Key::Date(d) => {
            // Используем JsDate::new + set_time (BOA_022 §9)
            let date = JsDate::new(context);
            date.set_time(JsValue::from(*d), context)?;
            Ok(date.into())
        }
        Key::String(s) => {
            // Используем utf16_to_js_string (BOA_022 §2)
            Ok(JsValue::from(super::boa_compat::utf16_to_js_string(s)))
        }
        Key::Binary(b) => {
            // Создаем настоящий ArrayBuffer (BOA_022 §8)
            let buf = create_array_buffer(b, context)?;
            Ok(buf.into())
        }
        Key::Array(arr) => {
            let elements: Vec<JsValue> = arr
                .iter()
                .map(|k| key_to_value(k, context))
                .collect::<JsResult<Vec<_>>>()?;
            let js_arr = JsArray::from_iter(elements, context);
            Ok(js_arr.into())
        }
    }
}
