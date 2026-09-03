//! `DOMException` implementation.

use boa_engine::{Context, JsNativeError, JsResult, JsValue};

use crate::convert::boa_compat::native_error_to_js_value;

/// DOMException name-to-code mapping.
pub fn dom_exception_code(name: &str) -> u16 {
    match name {
        "IndexSizeError" => 1,
        "HierarchyRequestError" => 3,
        "WrongDocumentError" => 4,
        "InvalidCharacterError" => 5,
        "NoModificationAllowedError" => 7,
        "NotFoundError" => 8,
        "NotSupportedError" => 9,
        "InUseAttributeError" => 10,
        "InvalidStateError" => 11,
        "SyntaxError" => 12,
        "InvalidModificationError" => 13,
        "NamespaceError" => 14,
        "InvalidAccessError" => 15,
        "TypeMismatchError" => 17,
        "SecurityError" => 18,
        "NetworkError" => 19,
        "AbortError" => 20,
        "URLMismatchError" => 21,
        "QuotaExceededError" => 22,
        "TimeoutError" => 23,
        "InvalidNodeTypeError" => 24,
        "DataCloneError" => 25,
        _ => 0,
    }
}

/// Creates a `DOMException` JsValue with the given name and message.
pub fn create_dom_exception(name: &str, message: &str, context: &mut Context) -> JsValue {
    let err = JsNativeError::error().with_message(format!("{name}: {message}"));
    native_error_to_js_value(err, context)
}

/// Creates a `ConstraintError` DOMException.
pub fn constraint_error(message: &str, context: &mut Context) -> JsValue {
    create_dom_exception("ConstraintError", message, context)
}

/// Creates a `DataError` DOMException.
pub fn data_error(message: &str, context: &mut Context) -> JsValue {
    create_dom_exception("DataError", message, context)
}

/// Creates a `DataCloneError` DOMException.
pub fn data_clone_error(message: &str, context: &mut Context) -> JsValue {
    create_dom_exception("DataCloneError", message, context)
}

/// Creates a `NotFoundError` DOMException.
pub fn not_found_error(message: &str, context: &mut Context) -> JsValue {
    create_dom_exception("NotFoundError", message, context)
}

/// Creates a `TransactionInactiveError` DOMException.
pub fn transaction_inactive_error(message: &str, context: &mut Context) -> JsValue {
    create_dom_exception("TransactionInactiveError", message, context)
}

/// Creates a `VersionError` DOMException.
pub fn version_error(message: &str, context: &mut Context) -> JsValue {
    create_dom_exception("VersionError", message, context)
}
