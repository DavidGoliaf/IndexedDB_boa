//! WebIDL type converters for IndexedDB arguments.

use boa_engine::{Context, JsNativeError, JsResult, JsValue};
use boa_idb_core::key::utf16::Utf16String;

/// Converts a `JsValue` to `u64` using `[EnforceRange]` rules.
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
pub fn to_dom_string(val: &JsValue, context: &mut Context) -> JsResult<Utf16String> {
    let js_str = val.to_string(context)?;
    let s = js_str.to_std_string_escaped();
    Ok(Utf16String::from_rust_str(&s))
}

/// Converts a boolean, defaulting to `false` if undefined.
pub fn to_boolean_default_false(val: &JsValue) -> bool {
    !val.is_undefined() && val.to_boolean()
}
