//! Converters between `JsValue` and `Key` (§7.3, §7.4).

use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::error::KeyError;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;

use super::boa_compat::{JsObjectExt, create_array_buffer, js_string_to_utf16};

/// Maximum array nesting depth for keys.
const MAX_KEY_DEPTH: usize = 32;

/// Converts a `JsValue` to a `Key` (§7.3).
pub fn value_to_key(val: &JsValue, context: &mut Context) -> Result<Key, KeyError> {
    value_to_key_depth(val, context, 0)
}

fn value_to_key_depth(val: &JsValue, context: &mut Context, depth: usize) -> Result<Key, KeyError> {
    if depth > MAX_KEY_DEPTH {
        return Err(KeyError::MaxDepthExceeded(MAX_KEY_DEPTH));
    }

    // Number — используем as_number() без аллокации
    if let Some(n) = val.as_number() {
        if n.is_nan() {
            return Err(KeyError::InvalidValue("NaN is not a valid key".into()));
        }
        return Ok(Key::Number(n));
    }

    // String — используем as_string() + to_vec() для сохранения суррогатов
    if let Some(s) = val.as_string() {
        return Ok(Key::String(js_string_to_utf16(&s)));
    }

    // Object (Date, Array, ArrayBuffer)
    if let Some(obj) = val.as_object() {
        // Check for Date — используем JsDate::from_object
        if let Ok(date) = boa_engine::object::builtins::JsDate::from_object(obj.clone()) {
            if let Ok(time_val) = date.get_time(context) {
                if let Some(d) = time_val.as_number() {
                    if d.is_nan() {
                        return Err(KeyError::InvalidValue("Date value is NaN".into()));
                    }
                    return Ok(Key::Date(d));
                }
            }
        }

        // Check for Array — используем JsArray::from_object
        if obj.is_array() {
            let length = obj
                .array_length(context)
                .map_err(|_| KeyError::InvalidType("Cannot get array length".into()))?
                as u32;

            let mut keys = Vec::with_capacity(length as usize);
            for i in 0..length {
                let elem = obj
                    .get(i, context)
                    .map_err(|_| KeyError::InvalidType("Cannot get array element".into()))?;
                if elem.is_undefined() {
                    // Проверяем, что это не hole, а явный undefined
                    if !obj.has_property(i, context).unwrap_or(false) {
                        return Err(KeyError::InvalidValue(
                            "Array holes are not allowed in keys".into(),
                        ));
                    }
                }
                let key = value_to_key_depth(&elem, context, depth + 1)?;
                keys.push(key);
            }
            return Ok(Key::Array(keys));
        }

        // Check for ArrayBuffer — используем JsArrayBuffer::from_object
        if let Ok(buf) = boa_engine::object::builtins::JsArrayBuffer::from_object(obj.clone()) {
            if let Some(data) = buf.data() {
                return Ok(Key::Binary(data.to_vec()));
            }
        }

        return Err(KeyError::InvalidType("Cannot convert object to key".into()));
    }

    // BigInt
    if val.is_bigint() {
        return Err(KeyError::InvalidType(
            "BigInt cannot be used as a key".into(),
        ));
    }

    Err(KeyError::InvalidType(format!(
        "Cannot convert {:?} to key",
        val.type_of()
    )))
}

/// Converts a `Key` to a `JsValue` (§7.4).
pub fn key_to_value(key: &Key, context: &mut Context) -> JsResult<JsValue> {
    match key {
        Key::Number(n) => Ok(JsValue::from(*n)),
        Key::Date(d) => {
            // Используем JsDate::new + set_time
            let date = boa_engine::object::builtins::JsDate::new(context);
            date.set_time(JsValue::from(*d), context)?;
            Ok(date.into())
        }
        Key::String(s) => {
            // Используем utf16_to_js_string
            Ok(JsValue::from(super::boa_compat::utf16_to_js_string(s)))
        }
        Key::Binary(b) => {
            // Создаем настоящий ArrayBuffer
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
