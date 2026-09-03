//! Structured cloning: `JsValue` ⇄ `ScValue` (§5.11).

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::clone::scvalue::{RegExpFlags, ScValue};
use boa_idb_core::key::utf16::Utf16String;
use indexmap::IndexMap;

use super::boa_compat::{JsObjectExt, js_string_to_utf16, utf16_to_js_string};

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

    // Примитивы — используем as_*() без аллокаций
    if val.is_undefined() {
        return Ok(ScValue::Undefined);
    }
    if val.is_null() {
        return Ok(ScValue::Null);
    }
    if let Some(b) = val.as_boolean() {
        return Ok(ScValue::Boolean(b));
    }
    if let Some(n) = val.as_number() {
        return Ok(ScValue::Number(n));
    }
    if let Some(s) = val.as_string() {
        return Ok(ScValue::String(js_string_to_utf16(&s)));
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

    // Объекты
    if let Some(obj) = val.as_object() {
        return serialize_object(&obj, context, depth);
    }

    Err(JsNativeError::typ()
        .with_message("DataCloneError: Unsupported type")
        .into())
}

fn serialize_object(obj: &JsObject, context: &mut Context, depth: usize) -> JsResult<ScValue> {
    // Date — используем JsDate::from_object
    if let Ok(date) = boa_engine::object::builtins::JsDate::from_object(obj.clone()) {
        if let Ok(time_val) = date.get_time(context) {
            if let Some(d) = time_val.as_number() {
                return Ok(ScValue::Date(d));
            }
        }
    }

    // RegExp
    if let Ok(has_source) = obj.has_property(js_string!("source"), context) {
        if has_source {
            if let Ok(source) = obj.get(js_string!("source"), context) {
                if let Some(s) = source.as_string() {
                    let pattern = js_string_to_utf16(&s);

                    let flags = if let Ok(flags_val) = obj.get(js_string!("flags"), context) {
                        if let Some(f) = flags_val.as_string() {
                            parse_regexp_flags(&f.to_std_string_escaped())
                        } else {
                            RegExpFlags::default()
                        }
                    } else {
                        RegExpFlags::default()
                    };

                    return Ok(ScValue::RegExp { pattern, flags });
                }
            }
        }
    }

    // Array — используем JsArray::from_object
    if obj.is_array() {
        let length = obj.array_length(context)? as u32;
        let mut elements = Vec::with_capacity(length as usize);
        for i in 0..length {
            let elem = obj.get(i, context)?;
            if elem.is_undefined() {
                let has_prop = obj.has_property(i, context)?;
                if has_prop {
                    elements.push(Some(serialize_depth(&elem, context, depth + 1)?));
                } else {
                    elements.push(None); // hole
                }
            } else {
                elements.push(Some(serialize_depth(&elem, context, depth + 1)?));
            }
        }
        return Ok(ScValue::Array {
            elements,
            extra_props: Vec::new(),
        });
    }

    // ArrayBuffer — используем JsArrayBuffer::from_object
    if let Ok(buf) = boa_engine::object::builtins::JsArrayBuffer::from_object(obj.clone()) {
        if let Some(data) = buf.data() {
            return Ok(ScValue::ArrayBuffer {
                data: data.to_vec(),
                max_byte_length: None,
            });
        }
    }

    // Default: plain Object — используем own_property_keys
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

fn parse_regexp_flags(flags: &str) -> RegExpFlags {
    let mut result = RegExpFlags::default();
    for ch in flags.chars() {
        match ch {
            'd' => result.has_indices = true,
            'g' => result.global = true,
            'i' => result.ignore_case = true,
            'm' => result.multiline = true,
            's' => result.dot_all = true,
            'u' => result.unicode = true,
            'v' => result.unicode_sets = true,
            'y' => result.sticky = true,
            _ => {}
        }
    }
    result
}

/// Deserializes an `ScValue` back to a `JsValue`.
pub fn deserialize_from_storage(val: &ScValue, context: &mut Context) -> JsResult<JsValue> {
    match val {
        ScValue::Undefined => Ok(JsValue::undefined()),
        ScValue::Null => Ok(JsValue::null()),
        ScValue::Boolean(b) => Ok(JsValue::from(*b)),
        ScValue::Number(n) => Ok(JsValue::from(*n)),
        ScValue::String(s) => Ok(JsValue::from(utf16_to_js_string(s))),
        ScValue::Date(d) => {
            // Используем JsDate::new + set_time
            let date = boa_engine::object::builtins::JsDate::new(context);
            date.set_time(JsValue::from(*d), context)?;
            Ok(date.into())
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
                let js_key = utf16_to_js_string(key);
                let js_val = deserialize_from_storage(value, context)?;
                obj.set(js_key, js_val, false, context)?;
            }
            Ok(obj.into())
        }
        ScValue::ArrayBuffer { data, .. } => {
            // Создаем настоящий ArrayBuffer
            let buf = super::boa_compat::create_array_buffer(data, context)?;
            Ok(buf.into())
        }
        _ => Err(JsNativeError::typ()
            .with_message("DataCloneError: Unsupported type for deserialization")
            .into()),
    }
}
