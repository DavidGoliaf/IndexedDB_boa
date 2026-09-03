//! Converters between `JsValue` and `Key` (§7.3, §7.4).

use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};
use boa_idb_core::error::KeyError;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;

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

    // Number
    if val.is_number() {
        let n = val
            .to_number(context)
            .map_err(|_| KeyError::InvalidType("Cannot convert to number".into()))?;
        if n.is_nan() {
            return Err(KeyError::InvalidValue("NaN is not a valid key".into()));
        }
        return Ok(Key::Number(n));
    }

    // String
    if val.is_string() {
        let s = val
            .to_string(context)
            .map_err(|_| KeyError::InvalidType("Cannot convert to string".into()))?;
        let rust_str = s.to_std_string_escaped();
        return Ok(Key::String(Utf16String::from_rust_str(&rust_str)));
    }

    // Object (Date, Array, etc.)
    if let Some(obj) = val.as_object() {
        // Check for Array
        if obj.is_array() {
            let length = obj
                .get(js_string!("length"), context)
                .map_err(|_| KeyError::InvalidType("Cannot get array length".into()))?
                .to_number(context)
                .map_err(|_| KeyError::InvalidType("length is not a number".into()))?
                as u32;

            let mut keys = Vec::with_capacity(length as usize);
            for i in 0..length {
                let elem = obj
                    .get(i, context)
                    .map_err(|_| KeyError::InvalidType("Cannot get array element".into()))?;
                if elem.is_undefined() {
                    return Err(KeyError::InvalidValue(
                        "Array holes are not allowed in keys".into(),
                    ));
                }
                let key = value_to_key_depth(&elem, context, depth + 1)?;
                keys.push(key);
            }
            return Ok(Key::Array(keys));
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
            // Create Date object
            let date_constructor = context.global_object().get(js_string!("Date"), context)?;
            let date = date_constructor
                .as_callable()
                .ok_or_else(|| JsNativeError::typ().with_message("Date is not callable"))?
                .call(&JsValue::undefined(), &[JsValue::from(*d)], context)?;
            Ok(date)
        }
        Key::String(s) => {
            let rust_str = s.to_string();
            Ok(JsValue::from(boa_engine::JsString::from(rust_str.as_str())))
        }
        Key::Binary(b) => {
            // Create Array from bytes (simplified)
            let arr = boa_engine::object::builtins::JsArray::from_iter(
                b.iter().map(|&byte| JsValue::from(byte)),
                context,
            );
            Ok(arr.into())
        }
        Key::Array(arr) => {
            let elements: Vec<JsValue> = arr
                .iter()
                .map(|k| key_to_value(k, context))
                .collect::<JsResult<Vec<_>>>()?;
            let js_arr = boa_engine::object::builtins::JsArray::from_iter(elements, context);
            Ok(js_arr.into())
        }
    }
}
