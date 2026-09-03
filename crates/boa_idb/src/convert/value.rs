//! Structured cloning: `JsValue` ⇄ `ScValue` (§5.11).

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::clone::scvalue::{RegExpFlags, ScValue};
use boa_idb_core::key::utf16::Utf16String;
use indexmap::IndexMap;

/// Serializes a `JsValue` to an `ScValue` for storage.
pub fn serialize_for_storage(val: &JsValue, context: &mut Context) -> JsResult<ScValue> {
    serialize_depth(val, context, 0)
}

fn serialize_depth(val: &JsValue, context: &mut Context, depth: usize) -> JsResult<ScValue> {
    if depth > 512 {
        return Err(JsNativeError::typ()
            .with_message("Maximum cloning depth exceeded")
            .into());
    }

    if val.is_undefined() {
        return Ok(ScValue::Undefined);
    }
    if val.is_null() {
        return Ok(ScValue::Null);
    }
    if val.is_boolean() {
        return Ok(ScValue::Boolean(val.to_boolean()));
    }
    if val.is_number() {
        return Ok(ScValue::Number(val.to_number(context)?));
    }
    if val.is_string() {
        let s = val.to_string(context)?;
        let rust_str = s.to_std_string_escaped();
        return Ok(ScValue::String(Utf16String::from_rust_str(&rust_str)));
    }
    if val.is_bigint() {
        return Err(JsNativeError::typ()
            .with_message("DataCloneError: BigInt is not supported")
            .into());
    }
    if val.is_symbol() {
        return Err(JsNativeError::typ()
            .with_message("DataCloneError: Symbol cannot be cloned")
            .into());
    }

    if let Some(obj) = val.as_object() {
        return serialize_object(&obj, context, depth);
    }

    Err(JsNativeError::typ()
        .with_message("DataCloneError: Unsupported type")
        .into())
}

fn serialize_object(obj: &JsObject, context: &mut Context, depth: usize) -> JsResult<ScValue> {
    // Check for Array
    if obj.is_array() {
        let length = obj.get(js_string!("length"), context)?.to_number(context)? as u32;
        let mut elements = Vec::with_capacity(length as usize);
        for i in 0..length {
            let elem = obj.get(i, context)?;
            if elem.is_undefined() {
                elements.push(None);
            } else {
                elements.push(Some(serialize_depth(&elem, context, depth + 1)?));
            }
        }
        return Ok(ScValue::Array {
            elements,
            extra_props: Vec::new(),
        });
    }

    // Default: plain Object
    let mut map = IndexMap::new();
    let keys = obj.own_property_keys(context)?;
    for key in keys {
        let val = obj.get(key.clone(), context)?;
        let key_str = key.to_string();
        let utf16_key = Utf16String::from_rust_str(&key_str);
        let serialized = serialize_depth(&val, context, depth + 1)?;
        map.insert(utf16_key, serialized);
    }

    Ok(ScValue::Object(map))
}

/// Deserializes an `ScValue` back to a `JsValue`.
pub fn deserialize_from_storage(val: &ScValue, context: &mut Context) -> JsResult<JsValue> {
    match val {
        ScValue::Undefined => Ok(JsValue::undefined()),
        ScValue::Null => Ok(JsValue::null()),
        ScValue::Boolean(b) => Ok(JsValue::from(*b)),
        ScValue::Number(n) => Ok(JsValue::from(*n)),
        ScValue::String(s) => {
            let rust_str = s.to_string();
            Ok(JsValue::from(boa_engine::JsString::from(rust_str.as_str())))
        }
        ScValue::Date(d) => {
            let date_constructor = context.global_object().get(js_string!("Date"), context)?;
            let date = date_constructor
                .as_callable()
                .ok_or_else(|| JsNativeError::typ().with_message("Date is not callable"))?
                .call(&JsValue::undefined(), &[JsValue::from(*d)], context)?;
            Ok(date)
        }
        ScValue::Array { elements, .. } => {
            let js_elements: Vec<JsValue> = elements
                .iter()
                .map(|elem| match elem {
                    Some(v) => deserialize_from_storage(v, context),
                    None => Ok(JsValue::undefined()),
                })
                .collect::<JsResult<Vec<_>>>()?;
            let arr = boa_engine::object::builtins::JsArray::from_iter(js_elements, context);
            Ok(arr.into())
        }
        ScValue::Object(map) => {
            let obj = JsObject::with_null_proto();
            for (key, value) in map {
                let rust_str = key.to_string();
                let js_key = boa_engine::JsString::from(rust_str.as_str());
                let js_val = deserialize_from_storage(value, context)?;
                obj.set(js_key, js_val, false, context)?;
            }
            Ok(obj.into())
        }
        _ => Err(JsNativeError::typ()
            .with_message("DataCloneError: Unsupported type for deserialization")
            .into()),
    }
}
