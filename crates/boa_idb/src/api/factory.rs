//! `IDBFactory` implementation.

use boa_engine::{Context, JsNativeError, JsResult, JsValue};

/// Compares two keys.
pub fn cmp(context: &mut Context, first: &JsValue, second: &JsValue) -> JsResult<i32> {
    use crate::convert::key::value_to_key;
    use boa_idb_core::key::compare::compare_keys;

    let k1 = value_to_key(first, context)
        .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;
    let k2 = value_to_key(second, context)
        .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;

    Ok(compare_keys(&k1, &k2) as i32)
}
