//! `IDBRequest` / `IDBOpenDBRequest` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};

/// Ready state for requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState {
    Pending,
    Done,
}

/// Native data for `IDBRequest`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBRequest {
    #[unsafe_ignore_trace]
    pub request_id: u64,
    #[unsafe_ignore_trace]
    pub ready_state: ReadyState,
    pub result: Option<JsValue>,
    pub error: Option<JsValue>,
    pub source: Option<JsObject>,
    pub transaction: Option<JsObject>,
}

impl IdBRequest {
    pub fn new(request_id: u64) -> Self {
        Self {
            request_id,
            ready_state: ReadyState::Pending,
            result: None,
            error: None,
            source: None,
            transaction: None,
        }
    }
}

impl Class for IdBRequest {
    const NAME: &'static str = "IDBRequest";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBRequest cannot be constructed directly")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        let result_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRequest>() {
                    return Ok(data.result.clone().unwrap_or(JsValue::undefined()));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let error_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRequest>() {
                    return Ok(data.error.clone().unwrap_or(JsValue::null()));
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        let ready_state_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRequest>() {
                    let state = match data.ready_state {
                        ReadyState::Pending => "pending",
                        ReadyState::Done => "done",
                    };
                    return Ok(JsValue::from(js_string!(state)));
                }
            }
            Ok(JsValue::from(js_string!("pending")))
        })
        .to_js_function(&realm);

        let source_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRequest>() {
                    if let Some(ref source) = data.source {
                        return Ok(JsValue::from(source.clone()));
                    }
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        let transaction_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRequest>() {
                    if let Some(ref txn) = data.transaction {
                        return Ok(JsValue::from(txn.clone()));
                    }
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("result"),
            Some(result_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("error"),
            Some(error_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("readyState"),
            Some(ready_state_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("source"),
            Some(source_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("transaction"),
            Some(transaction_getter),
            None,
            Attribute::READONLY,
        );

        class.accessor(
            js_string!("onsuccess"),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::null()))
                    .to_js_function(&realm),
            ),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()))
                    .to_js_function(&realm),
            ),
            Attribute::all(),
        );

        class.accessor(
            js_string!("onerror"),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::null()))
                    .to_js_function(&realm),
            ),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()))
                    .to_js_function(&realm),
            ),
            Attribute::all(),
        );

        Ok(())
    }
}

/// `IDBOpenDBRequest` class (extends IDBRequest).
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBOpenDBRequest;

impl Class for IdBOpenDBRequest {
    const NAME: &'static str = "IDBOpenDBRequest";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBOpenDBRequest cannot be constructed directly")
            .into())
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        class.accessor(
            js_string!("onblocked"),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::null()))
                    .to_js_function(&realm),
            ),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()))
                    .to_js_function(&realm),
            ),
            Attribute::all(),
        );

        class.accessor(
            js_string!("onupgradeneeded"),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::null()))
                    .to_js_function(&realm),
            ),
            Some(
                NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()))
                    .to_js_function(&realm),
            ),
            Attribute::all(),
        );

        Ok(())
    }
}
