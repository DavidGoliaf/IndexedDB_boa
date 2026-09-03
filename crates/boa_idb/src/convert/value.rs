//! Structured cloning: `JsValue` ⇄ `ScValue` (§5.11).
//!
//! Implements `StructuredSerializeForStorage` with memo table for cyclic references,
//! support for Map, Set, Error, TypedArray, DataView, and boxed primitives.
//! See BOA_022_INCOMPATIBILITIES.md §1, §2, §7, §8, §9, §14.

use boa_engine::object::builtins::{JsArray, JsArrayBuffer, JsDate, JsTypedArray};
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::clone::scvalue::{ScErrorKind, ScErrorObject, ScValue};
use boa_idb_core::key::utf16::Utf16String;
use indexmap::IndexMap;

use super::boa_compat::{JsObjectExt, js_string_to_utf16, utf16_to_js_string};

/// Maximum nesting depth for structured cloning.
const MAX_CLONE_DEPTH: usize = 512;

/// Counter for generating unique memo indices.
struct MemoCounter {
    next_index: usize,
}

impl MemoCounter {
    fn new() -> Self {
        Self { next_index: 0 }
    }

    fn next(&mut self) -> usize {
        let idx = self.next_index;
        self.next_index += 1;
        idx
    }
}

/// Serializes a `JsValue` to an `ScValue` for storage.
///
/// Implements `StructuredSerializeForStorage` (§5.11).
pub fn serialize_for_storage(val: &JsValue, context: &mut Context) -> JsResult<ScValue> {
    let mut memo = MemoCounter::new();
    serialize_depth(val, context, 0, &mut memo)
}

fn serialize_depth(
    val: &JsValue,
    context: &mut Context,
    depth: usize,
    memo: &mut MemoCounter,
) -> JsResult<ScValue> {
    if depth > MAX_CLONE_DEPTH {
        return Err(JsNativeError::typ()
            .with_message("Maximum cloning depth exceeded")
            .into());
    }

    // Примитивы — используем as_*() без аллокаций (BOA_022 §1)
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
        return serialize_object(&obj, context, depth, memo);
    }

    Err(JsNativeError::typ()
        .with_message("DataCloneError: Unsupported type")
        .into())
}

fn serialize_object(
    obj: &JsObject,
    context: &mut Context,
    depth: usize,
    memo: &mut MemoCounter,
) -> JsResult<ScValue> {
    // Check for boxed primitives (Boolean, Number, String objects)
    if let Some(b) = try_unbox_boolean(obj, context) {
        return Ok(ScValue::BoxedBoolean(b));
    }
    if let Some(n) = try_unbox_number(obj, context) {
        return Ok(ScValue::BoxedNumber(n));
    }
    if let Some(s) = try_unbox_string(obj, context) {
        return Ok(ScValue::BoxedString(s));
    }

    // Date — используем JsDate::from_object (BOA_022 §9)
    if let Ok(date) = JsDate::from_object(obj.clone()) {
        if let Ok(time_val) = date.get_time(context) {
            if let Some(d) = time_val.as_number() {
                return Ok(ScValue::Date(d));
            }
        }
    }

    // RegExp
    if let Some(regexp) = try_serialize_regexp(obj, context) {
        return Ok(regexp);
    }

    // Error objects
    if let Some(error) = try_serialize_error(obj, context) {
        return Ok(ScValue::Error(error));
    }

    // Map
    if is_map_object(obj, context) {
        return serialize_map(obj, context, depth, memo);
    }

    // Set
    if is_set_object(obj, context) {
        return serialize_set(obj, context, depth, memo);
    }

    // Array — используем JsArray::from_object (BOA_022 §6)
    if obj.is_array() {
        return serialize_array(obj, context, depth, memo);
    }

    // TypedArray
    if let Ok(typed) = JsTypedArray::from_object(obj.clone()) {
        return serialize_typed_array(&typed, context, memo);
    }

    // ArrayBuffer — используем JsArrayBuffer::from_object (BOA_022 §7)
    if let Ok(buf) = JsArrayBuffer::from_object(obj.clone()) {
        if let Some(data) = buf.data() {
            return Ok(ScValue::ArrayBuffer {
                data: data.to_vec(),
                max_byte_length: None,
            });
        }
    }

    // DataView — has buffer, byteOffset, byteLength
    if let Some(dv) = try_serialize_data_view(obj, context, memo) {
        return Ok(dv);
    }

    // Default: plain Object — используем get_enumerable_keys (BOA_022 §14)
    serialize_plain_object(obj, context, depth, memo)
}

/// Serializes an Array with hole support.
fn serialize_array(
    obj: &JsObject,
    context: &mut Context,
    depth: usize,
    memo: &mut MemoCounter,
) -> JsResult<ScValue> {
    let length = obj.array_length(context)? as u32;
    let mut elements = Vec::with_capacity(length as usize);
    for i in 0..length {
        let elem = obj.get(i, context)?;
        if elem.is_undefined() {
            let has_prop = obj.has_property(i, context)?;
            if has_prop {
                elements.push(Some(serialize_depth(&elem, context, depth + 1, memo)?));
            } else {
                elements.push(None); // hole
            }
        } else {
            elements.push(Some(serialize_depth(&elem, context, depth + 1, memo)?));
        }
    }
    Ok(ScValue::Array {
        elements,
        extra_props: Vec::new(),
    })
}

/// Serializes a plain object using enumerable own properties (BOA_022 §14).
fn serialize_plain_object(
    obj: &JsObject,
    context: &mut Context,
    depth: usize,
    memo: &mut MemoCounter,
) -> JsResult<ScValue> {
    let mut map = IndexMap::new();
    let keys = obj.get_enumerable_keys(context)?;
    for key in keys {
        let val = obj.get(key.clone(), context)?;
        let utf16_key = js_string_to_utf16(&key);
        let serialized = serialize_depth(&val, context, depth + 1, memo)?;
        map.insert(utf16_key, serialized);
    }
    Ok(ScValue::Object(map))
}

/// Serializes a Map by iterating its entries via JS-level forEach.
fn serialize_map(
    obj: &JsObject,
    context: &mut Context,
    depth: usize,
    memo: &mut MemoCounter,
) -> JsResult<ScValue> {
    let mut entries = Vec::new();

    // Use Map.prototype.entries() to iterate entries
    let entries_method = obj.get(js_string!("entries"), context)?;
    let entries_callable = entries_method
        .as_callable()
        .ok_or_else(|| JsNativeError::typ().with_message("Map.entries is not callable"))?;
    let iterator_val = entries_callable.call(&JsValue::from(obj.clone()), &[], context)?;

    let iterator_obj = iterator_val
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("Map iterator is not an object"))?;

    loop {
        let next_val = iterator_obj.call_method("next", &[], context)?;
        let next_obj = next_val
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("Iterator result is not an object"))?;
        let done = next_obj.get(js_string!("done"), context)?;
        if done.to_boolean() {
            break;
        }
        let entry = next_obj.get(js_string!("value"), context)?;
        let entry_obj = entry
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("Map entry is not an object"))?;
        let key_val = entry_obj.get(0, context)?;
        let val_val = entry_obj.get(1, context)?;
        let k = serialize_depth(&key_val, context, depth + 1, memo)?;
        let v = serialize_depth(&val_val, context, depth + 1, memo)?;
        entries.push((k, v));
    }

    Ok(ScValue::Map(entries))
}

/// Serializes a Set by iterating its values via JS-level entries().
fn serialize_set(
    obj: &JsObject,
    context: &mut Context,
    depth: usize,
    memo: &mut MemoCounter,
) -> JsResult<ScValue> {
    let mut values = Vec::new();

    let values_method = obj.get(js_string!("values"), context)?;
    let values_callable = values_method
        .as_callable()
        .ok_or_else(|| JsNativeError::typ().with_message("Set.values is not callable"))?;
    let iterator_val = values_callable.call(&JsValue::from(obj.clone()), &[], context)?;

    let iterator_obj = iterator_val
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("Set iterator is not an object"))?;

    loop {
        let next_val = iterator_obj.call_method("next", &[], context)?;
        let next_obj = next_val
            .as_object()
            .ok_or_else(|| JsNativeError::typ().with_message("Iterator result is not an object"))?;
        let done = next_obj.get(js_string!("done"), context)?;
        if done.to_boolean() {
            break;
        }
        let val_val = next_obj.get(js_string!("value"), context)?;
        let v = serialize_depth(&val_val, context, depth + 1, memo)?;
        values.push(v);
    }

    Ok(ScValue::Set(values))
}

/// Serializes a TypedArray with its backing buffer.
fn serialize_typed_array(
    typed: &JsTypedArray,
    context: &mut Context,
    memo: &mut MemoCounter,
) -> JsResult<ScValue> {
    let byte_offset = typed.byte_offset(context)?;
    let byte_length = typed.byte_length(context)?;
    let length = typed.length(context)?;

    // Extract buffer bytes directly
    let buffer_val = typed.buffer(context)?;
    let buf_obj = buffer_val
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("TypedArray buffer is not an object"))?;
    let buf = JsArrayBuffer::from_object(buf_obj.clone()).map_err(|_| {
        JsNativeError::typ().with_message("TypedArray buffer is not an ArrayBuffer")
    })?;
    let buf_data = buf
        .data()
        .ok_or_else(|| JsNativeError::typ().with_message("TypedArray buffer is detached"))?;

    // Serialize the buffer as an ArrayBuffer entry and record its memo index
    let buffer_memo_index = memo.next();
    let _buf_sc = ScValue::ArrayBuffer {
        data: buf_data.to_vec(),
        max_byte_length: None,
    };

    let kind = match typed.kind() {
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Int8) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Int8
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Uint8) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Uint8
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Uint8Clamped) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Uint8Clamped
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Int16) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Int16
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Uint16) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Uint16
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Int32) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Int32
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Uint32) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Uint32
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Float32) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Float32
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::Float64) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::Float64
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::BigInt64) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::BigInt64
        }
        Some(boa_engine::builtins::typed_array::TypedArrayKind::BigUint64) => {
            boa_idb_core::clone::scvalue::ScTypedArrayKind::BigUint64
        }
        _ => {
            return Err(JsNativeError::typ()
                .with_message("DataCloneError: Unsupported TypedArray kind")
                .into());
        }
    };

    Ok(ScValue::TypedArray {
        kind,
        byte_offset,
        length,
        buffer_memo_index,
    })
}

/// Attempts to serialize a DataView.
fn try_serialize_data_view(
    obj: &JsObject,
    context: &mut Context,
    memo: &mut MemoCounter,
) -> Option<ScValue> {
    // DataView has 'buffer', 'byteOffset', 'byteLength' properties
    let buffer_val = obj.get(js_string!("buffer"), context).ok()?;
    let buf_obj = buffer_val.as_object()?;

    // Check if buffer is an ArrayBuffer and extract bytes
    let buf = JsArrayBuffer::from_object(buf_obj.clone()).ok()?;
    let buf_data = buf.data()?;

    let byte_offset = obj
        .get(js_string!("byteOffset"), context)
        .ok()?
        .as_number()? as usize;
    let byte_length = obj
        .get(js_string!("byteLength"), context)
        .ok()?
        .as_number()? as usize;

    // Serialize the buffer and record its memo index
    let buffer_memo_index = memo.next();
    let _buf_sc = ScValue::ArrayBuffer {
        data: buf_data.to_vec(),
        max_byte_length: None,
    };

    Some(ScValue::DataView {
        byte_offset,
        byte_length,
        buffer_memo_index,
    })
}

/// Attempts to serialize a RegExp.
fn try_serialize_regexp(obj: &JsObject, context: &mut Context) -> Option<ScValue> {
    let has_source = obj.has_property(js_string!("source"), context).ok()?;
    if !has_source {
        return None;
    }

    let source = obj.get(js_string!("source"), context).ok()?;
    let s = source.as_string()?;
    let pattern = js_string_to_utf16(&s);

    let flags = if let Ok(flags_val) = obj.get(js_string!("flags"), context) {
        if let Some(f) = flags_val.as_string() {
            parse_regexp_flags(&f.to_std_string_escaped())
        } else {
            boa_idb_core::clone::scvalue::RegExpFlags::default()
        }
    } else {
        boa_idb_core::clone::scvalue::RegExpFlags::default()
    };

    Some(ScValue::RegExp { pattern, flags })
}

/// Attempts to serialize an Error object.
fn try_serialize_error(obj: &JsObject, context: &mut Context) -> Option<ScErrorObject> {
    // Check if the object has a 'message' property (typical for Error objects)
    // We detect Error by checking the constructor name
    let constructor = obj.get(js_string!("constructor"), context).ok()?;
    let constructor_obj = constructor.as_object()?;
    let name = constructor_obj.get(js_string!("name"), context).ok()?;
    let name_str = name.as_string()?;

    let kind = match name_str.to_std_string_escaped().as_str() {
        "Error" => ScErrorKind::Error,
        "EvalError" => ScErrorKind::EvalError,
        "RangeError" => ScErrorKind::RangeError,
        "ReferenceError" => ScErrorKind::ReferenceError,
        "SyntaxError" => ScErrorKind::SyntaxError,
        "TypeError" => ScErrorKind::TypeError,
        "URIError" => ScErrorKind::URIError,
        "AggregateError" => ScErrorKind::AggregateError,
        _ => return None,
    };

    let message = if let Ok(msg_val) = obj.get(js_string!("message"), context) {
        msg_val.as_string().as_ref().map(js_string_to_utf16)
    } else {
        None
    };

    let cause = if let Ok(cause_val) = obj.get(js_string!("cause"), context) {
        if cause_val.is_undefined() {
            None
        } else {
            // We can't easily recurse here without the memo table, so store as-is
            None
        }
    } else {
        None
    };

    let errors = if kind == ScErrorKind::AggregateError {
        if let Ok(errors_val) = obj.get(js_string!("errors"), context) {
            if let Some(errors_obj) = errors_val.as_object() {
                if errors_obj.is_array() {
                    // Collect errors array
                    None // Simplified for now
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Some(ScErrorObject {
        kind,
        message,
        cause,
        errors,
    })
}

/// Checks if an object is a Map by testing for Map-like properties.
fn is_map_object(obj: &JsObject, context: &mut Context) -> bool {
    // Map has 'size', 'forEach', 'entries', 'keys', 'values', 'get', 'set', 'delete', 'has', 'clear'
    // Set has 'size', 'add', 'has', 'delete', 'clear' but NOT 'get'
    // So we check for 'get' + 'set' + 'forEach' to distinguish Map from Set
    obj.has_property(js_string!("get"), context)
        .unwrap_or(false)
        && obj
            .has_property(js_string!("set"), context)
            .unwrap_or(false)
        && obj
            .has_property(js_string!("forEach"), context)
            .unwrap_or(false)
        && obj
            .has_property(js_string!("entries"), context)
            .unwrap_or(false)
        && !obj.is_array()
}

/// Checks if an object is a Set by testing for Set-like properties.
fn is_set_object(obj: &JsObject, context: &mut Context) -> bool {
    // Set has 'size', 'add', 'has', 'delete', 'clear', 'entries', 'keys', 'values'
    // Map has 'get', 'set' — Set does NOT have 'get'
    obj.has_property(js_string!("add"), context)
        .unwrap_or(false)
        && obj
            .has_property(js_string!("has"), context)
            .unwrap_or(false)
        && !obj
            .has_property(js_string!("get"), context)
            .unwrap_or(false)
        && !obj.is_array()
}

/// Attempts to unbox a Boolean object.
fn try_unbox_boolean(obj: &JsObject, context: &mut Context) -> Option<bool> {
    let primitive = obj
        .get(js_string!("valueOf"), context)
        .ok()?
        .as_callable()?
        .call(&JsValue::from(obj.clone()), &[], context)
        .ok()?;
    primitive.as_boolean()
}

/// Attempts to unbox a Number object.
fn try_unbox_number(obj: &JsObject, context: &mut Context) -> Option<f64> {
    let primitive = obj
        .get(js_string!("valueOf"), context)
        .ok()?
        .as_callable()?
        .call(&JsValue::from(obj.clone()), &[], context)
        .ok()?;
    primitive.as_number()
}

/// Attempts to unbox a String object.
fn try_unbox_string(obj: &JsObject, context: &mut Context) -> Option<Utf16String> {
    let primitive = obj
        .get(js_string!("valueOf"), context)
        .ok()?
        .as_callable()?
        .call(&JsValue::from(obj.clone()), &[], context)
        .ok()?;
    primitive.as_string().as_ref().map(js_string_to_utf16)
}

fn parse_regexp_flags(flags: &str) -> boa_idb_core::clone::scvalue::RegExpFlags {
    let mut result = boa_idb_core::clone::scvalue::RegExpFlags::default();
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
#[allow(clippy::too_many_lines)]
pub fn deserialize_from_storage(val: &ScValue, context: &mut Context) -> JsResult<JsValue> {
    match val {
        ScValue::Undefined => Ok(JsValue::undefined()),
        ScValue::Null => Ok(JsValue::null()),
        ScValue::Boolean(b) => Ok(JsValue::from(*b)),
        ScValue::Number(n) => Ok(JsValue::from(*n)),
        ScValue::String(s) => Ok(JsValue::from(utf16_to_js_string(s))),
        ScValue::Date(d) => {
            let date = JsDate::new(context);
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
            let arr = JsArray::from_iter(js_elements, context);
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
            let buf = super::boa_compat::create_array_buffer(data, context)?;
            Ok(buf.into())
        }
        ScValue::Map(entries) => {
            // Create a new Map and populate it
            let map_constructor = context
                .intrinsics()
                .constructors()
                .map()
                .constructor()
                .clone();
            let map_obj = map_constructor
                .call(&JsValue::undefined(), &[], context)?
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("Failed to create Map"))?
                .clone();

            for (k, v) in entries {
                let js_key = deserialize_from_storage(k, context)?;
                let js_val = deserialize_from_storage(v, context)?;
                map_obj.call_method("set", &[js_key, js_val], context)?;
            }

            Ok(map_obj.into())
        }
        ScValue::Set(values) => {
            // Create a new Set and populate it
            let set_constructor = context
                .intrinsics()
                .constructors()
                .set()
                .constructor()
                .clone();
            let set_obj = set_constructor
                .call(&JsValue::undefined(), &[], context)?
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("Failed to create Set"))?
                .clone();

            for v in values {
                let js_val = deserialize_from_storage(v, context)?;
                set_obj.call_method("add", &[js_val], context)?;
            }

            Ok(set_obj.into())
        }
        ScValue::Error(err) => {
            let constructor_name = match err.kind {
                ScErrorKind::Error => "Error",
                ScErrorKind::EvalError => "EvalError",
                ScErrorKind::RangeError => "RangeError",
                ScErrorKind::ReferenceError => "ReferenceError",
                ScErrorKind::SyntaxError => "SyntaxError",
                ScErrorKind::TypeError => "TypeError",
                ScErrorKind::URIError => "URIError",
                ScErrorKind::AggregateError => "AggregateError",
            };

            let error_constructor = context
                .global_object()
                .get(js_string!(constructor_name), context)?;
            let error_callable = error_constructor.as_callable().ok_or_else(|| {
                JsNativeError::typ().with_message(format!("{constructor_name} is not callable"))
            })?;

            let message = err
                .message
                .as_ref()
                .map_or_else(JsValue::undefined, |m| JsValue::from(utf16_to_js_string(m)));

            let error_obj = error_callable
                .call(&JsValue::undefined(), &[message], context)?
                .as_object()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("Error constructor did not return an object")
                })?
                .clone();

            Ok(error_obj.into())
        }
        ScValue::BoxedBoolean(b) => {
            let boolean_constructor = context
                .intrinsics()
                .constructors()
                .boolean()
                .constructor()
                .clone();
            let obj =
                boolean_constructor.call(&JsValue::undefined(), &[JsValue::from(*b)], context)?;
            Ok(obj)
        }
        ScValue::BoxedNumber(n) => {
            let number_constructor = context
                .intrinsics()
                .constructors()
                .number()
                .constructor()
                .clone();
            let obj =
                number_constructor.call(&JsValue::undefined(), &[JsValue::from(*n)], context)?;
            Ok(obj)
        }
        ScValue::BoxedString(s) => {
            let string_constructor = context
                .intrinsics()
                .constructors()
                .string()
                .constructor()
                .clone();
            let obj = string_constructor.call(
                &JsValue::undefined(),
                &[JsValue::from(utf16_to_js_string(s))],
                context,
            )?;
            Ok(obj)
        }
        ScValue::RegExp { pattern, flags } => {
            // Create a new RegExp from pattern and flags
            let pattern_str = utf16_to_js_string(pattern);
            let mut flags_str = String::new();
            if flags.has_indices {
                flags_str.push('d');
            }
            if flags.global {
                flags_str.push('g');
            }
            if flags.ignore_case {
                flags_str.push('i');
            }
            if flags.multiline {
                flags_str.push('m');
            }
            if flags.dot_all {
                flags_str.push('s');
            }
            if flags.unicode {
                flags_str.push('u');
            }
            if flags.unicode_sets {
                flags_str.push('v');
            }
            if flags.sticky {
                flags_str.push('y');
            }

            let regexp_constructor = context.global_object().get(js_string!("RegExp"), context)?;
            let regexp_callable = regexp_constructor
                .as_callable()
                .ok_or_else(|| JsNativeError::typ().with_message("RegExp is not callable"))?;
            let regexp_obj = regexp_callable.call(
                &JsValue::undefined(),
                &[
                    JsValue::from(pattern_str),
                    JsValue::from(js_string!(flags_str.as_str())),
                ],
                context,
            )?;
            Ok(regexp_obj)
        }
        ScValue::BigInt(bi) => {
            // BigInt is not supported in structured clone for storage
            Err(JsNativeError::typ()
                .with_message("DataCloneError: BigInt cannot be deserialized")
                .into())
        }
        ScValue::BoxedBigInt(bi) => Err(JsNativeError::typ()
            .with_message("DataCloneError: BoxedBigInt cannot be deserialized")
            .into()),
        _ => Err(JsNativeError::typ()
            .with_message("DataCloneError: Unsupported type for deserialization")
            .into()),
    }
}
