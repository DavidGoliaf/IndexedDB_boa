//! `DOMException` implementation per WebIDL.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsNativeError, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};

/// Native data for `DOMException`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct DomException {
    #[unsafe_ignore_trace]
    pub name: String,
    #[unsafe_ignore_trace]
    pub message: String,
    #[unsafe_ignore_trace]
    pub code: u16,
}

impl DomException {
    pub fn new(name: impl Into<String>, message: impl Into<String>) -> Self {
        let name = name.into();
        let code = dom_exception_code(&name);
        Self {
            name,
            message: message.into(),
            code,
        }
    }
}

impl Class for DomException {
    const NAME: &'static str = "DOMException";
    const LENGTH: usize = 2;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<Self> {
        let message = if args.first().is_some_and(|v| !v.is_undefined()) {
            args[0].to_string(context)?.to_std_string_escaped()
        } else {
            String::new()
        };
        let name = if args.get(1).is_some_and(|v| !v.is_undefined()) {
            args[1].to_string(context)?.to_std_string_escaped()
        } else {
            "Error".to_string()
        };
        Ok(DomException::new(name, message))
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        let name_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<DomException>() {
                    return Ok(JsValue::from(js_string!(data.name.as_str())));
                }
            }
            Err(JsNativeError::typ()
                .with_message("'this' is not a DOMException")
                .into())
        })
        .to_js_function(&realm);

        let message_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<DomException>() {
                    return Ok(JsValue::from(js_string!(data.message.as_str())));
                }
            }
            Err(JsNativeError::typ()
                .with_message("'this' is not a DOMException")
                .into())
        })
        .to_js_function(&realm);

        let code_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<DomException>() {
                    return Ok(JsValue::from(data.code));
                }
            }
            Err(JsNativeError::typ()
                .with_message("'this' is not a DOMException")
                .into())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("name"),
            Some(name_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("message"),
            Some(message_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("code"),
            Some(code_getter),
            None,
            Attribute::READONLY,
        );

        Ok(())
    }
}

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

pub fn create_dom_exception(name: &str, message: &str, context: &mut Context) -> JsResult<JsValue> {
    let data = DomException::new(name, message);
    let obj = DomException::from_data(data, context)?;
    Ok(obj.into())
}

pub fn constraint_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("ConstraintError", message, context)
}

pub fn data_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("DataError", message, context)
}

pub fn data_clone_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("DataCloneError", message, context)
}

pub fn not_found_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("NotFoundError", message, context)
}

pub fn transaction_inactive_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("TransactionInactiveError", message, context)
}

pub fn version_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("VersionError", message, context)
}

pub fn invalid_state_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("InvalidStateError", message, context)
}

/// Throws a `DataError` DOMException.
pub fn throw_data_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    let value = data_error(message, context)?;
    Err(boa_engine::JsError::from_opaque(value))
}

/// Throws a `ConstraintError` DOMException.
pub fn throw_constraint_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    let value = constraint_error(message, context)?;
    Err(boa_engine::JsError::from_opaque(value))
}

/// Throws an `InvalidStateError` DOMException.
pub fn throw_invalid_state_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    let value = invalid_state_error(message, context)?;
    Err(boa_engine::JsError::from_opaque(value))
}

/// Throws a `TransactionInactiveError` DOMException.
pub fn throw_transaction_inactive_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    let value = transaction_inactive_error(message, context)?;
    Err(boa_engine::JsError::from_opaque(value))
}

/// Throws a `NotFoundError` DOMException.
pub fn throw_not_found_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    let value = not_found_error(message, context)?;
    Err(boa_engine::JsError::from_opaque(value))
}

/// Throws a `VersionError` DOMException.
pub fn throw_version_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    let value = version_error(message, context)?;
    Err(boa_engine::JsError::from_opaque(value))
}

/// Throws the matching DOMException for an [`IdbError`][boa_idb_core::error::IdbError].
///
/// Returns the throw error directly (not wrapped in `Err`): use as
/// `return Err(throw_idb_error(&e, context))` or
/// `.map_err(|e| throw_idb_error(&e, context))?`.
pub fn throw_idb_error(error: &boa_idb_core::error::IdbError, context: &mut Context) -> JsError {
    use boa_idb_core::error::IdbError as E;
    let name = match error {
        E::Abort => "AbortError",
        E::Constraint(_) => "ConstraintError",
        E::DataClone(_) => "DataCloneError",
        E::Data(_) => "DataError",
        E::InvalidAccess(_) => "InvalidAccessError",
        E::InvalidState(_) => "InvalidStateError",
        E::NotFound(_) => "NotFoundError",
        E::NotReadable(_) => "NotReadableError",
        E::Syntax(_) => "SyntaxError",
        E::ReadOnly => "ReadOnlyError",
        E::TransactionInactive => "TransactionInactiveError",
        E::Unknown(_) => "UnknownError",
        E::Version(_) => "VersionError",
        E::QuotaExceeded { .. } => "QuotaExceededError",
        // `IdbError` is non-exhaustive: future variants map here.
        _ => "UnknownError",
    };
    match create_dom_exception(name, &error.to_string(), context) {
        Ok(value) => boa_engine::JsError::from_opaque(value),
        Err(e) => e,
    }
}

/// Throws a `TypeError` DOMException.
pub fn throw_type_error(message: &str) -> JsError {
    JsNativeError::typ()
        .with_message(message.to_string())
        .into()
}

pub fn read_only_error(message: &str, context: &mut Context) -> JsResult<JsValue> {
    create_dom_exception("ReadOnlyError", message, context)
}
