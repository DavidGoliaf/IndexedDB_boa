//! `DOMException` implementation.

use boa_engine::{Context, JsNativeError, JsResult, JsValue};

/// DOMException name-to-code mapping.
pub fn dom_exception_code(name: &str) -> u16 {
    match name {
        "AbortError" => 20,
        "ConstraintError" => 0,
        "DataCloneError" => 25,
        "DataError" => 0,
        "InvalidStateError" => 11,
        "NotFoundError" => 8,
        "NotSupportedError" => 9,
        "QuotaExceededError" => 22,
        "SyntaxError" => 12,
        "TransactionInactiveError" => 0,
        "VersionError" => 0,
        _ => 0,
    }
}

/// Creates a `DOMException` with the given name and message.
pub fn create_dom_exception(name: &str, message: &str, context: &mut Context) -> JsResult<JsValue> {
    let err = JsNativeError::error().with_message(format!("{name}: {message}"));
    Err(err.into())
}
