//! `IDBRequest` / `IDBOpenDBRequest` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, GcRefCell, Trace};
use std::collections::HashMap;

/// Ready state for requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState {
    /// Request queued, not yet run.
    Pending,
    /// Request finished.
    Done,
}

/// Native data for `IDBRequest`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBRequest {
    /// Request id issued by the driver.
    #[unsafe_ignore_trace]
    pub request_id: u64,
    /// Ready state.
    #[unsafe_ignore_trace]
    pub ready_state: ReadyState,
    /// Result value (`null` before done).
    pub result: Option<JsValue>,
    /// Error value (`null` before done).
    pub error: Option<JsValue>,
    /// Source object (object store / index / cursor).
    pub source: Option<JsObject>,
    /// Owning transaction object.
    pub transaction: Option<JsObject>,
    /// Versionchange transaction for open requests (== transaction during upgrade).
    pub upgrade_txn: Option<JsObject>,
    /// Listeners registered through `addEventListener`.
    pub listeners: GcRefCell<Vec<crate::dom::event_target::EventListenerEntry>>,
    /// Attribute handlers (`onsuccess`, `onerror`, …).
    pub attr_handlers: GcRefCell<HashMap<String, JsValue>>,
}

impl IdBRequest {
    /// Creates a new request shell with a driver-issued id.
    pub fn new_with_id(request_id: u64) -> Self {
        Self {
            request_id,
            ready_state: ReadyState::Pending,
            result: None,
            error: None,
            source: None,
            transaction: None,
            upgrade_txn: None,
            listeners: GcRefCell::default(),
            attr_handlers: GcRefCell::default(),
        }
    }

    /// Creates a request and returns its JS object.
    pub fn create_object(
        request_id: u64,
        source: Option<JsObject>,
        transaction: Option<JsObject>,
        context: &mut Context,
    ) -> JsResult<JsObject> {
        let data = Self::new_with_id(request_id);
        let obj = Self::from_data(data, context)?;
        if let Some(mut d) = obj.downcast_mut::<IdBRequest>() {
            d.source = source;
            d.transaction = transaction;
        }
        crate::runtime::register_request(context, request_id, obj.clone());
        Ok(obj)
    }

    /// Sets a handler attribute (`onsuccess` → event name `success`).
    pub fn set_handler(&self, name: &str, value: JsValue) {
        self.attr_handlers
            .borrow_mut()
            .insert(name.to_string(), value);
    }
}

/// Runs `f` against the `IdBRequest` data of a request object.
///
/// Open requests are `IDBOpenDBRequest` objects wrapping an inner
/// `IdBRequest`; plain requests carry it directly.
pub fn with_request_mut<R>(obj: &JsObject, f: impl FnOnce(&mut IdBRequest) -> R) -> Option<R> {
    if let Some(mut outer) = obj.downcast_mut::<IdBOpenDBRequest>() {
        return Some(f(&mut outer.request));
    }
    if let Some(mut data) = obj.downcast_mut::<IdBRequest>() {
        return Some(f(&mut data));
    }
    None
}

/// Reads `IdBRequest` data of a request object.
pub fn with_request_ref<R>(obj: &JsObject, f: impl FnOnce(&IdBRequest) -> R) -> Option<R> {
    if let Some(outer) = obj.downcast_ref::<IdBOpenDBRequest>() {
        return Some(f(&outer.request));
    }
    if let Some(data) = obj.downcast_ref::<IdBRequest>() {
        return Some(f(&data));
    }
    None
}

/// Creates an `IDBOpenDBRequest` shell with a driver-issued id and registers it.
pub fn create_open_request(request_id: u64, context: &mut Context) -> JsResult<JsObject> {
    let outer = IdBOpenDBRequest {
        request: IdBRequest::new_with_id(request_id),
    };
    let obj = IdBOpenDBRequest::from_data(outer, context)?;
    crate::runtime::register_request(context, request_id, obj.clone());
    Ok(obj)
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
        install_request_class(class)?;
        install_event_listener_methods(class)?;
        Ok(())
    }
}

/// `IDBOpenDBRequest` class (extends IDBRequest).
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBOpenDBRequest {
    /// Underlying request data.
    pub request: IdBRequest,
}

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
        install_request_class(class)?;
        install_event_listener_methods(class)?;

        let realm = class.context().realm().clone();
        class.accessor(
            js_string!("onblocked"),
            Some(handler_accessor(&realm, "blocked")),
            Some(handler_setter(&realm, "blocked")),
            Attribute::all(),
        );
        class.accessor(
            js_string!("onupgradeneeded"),
            Some(handler_accessor(&realm, "upgradeneeded")),
            Some(handler_setter(&realm, "upgradeneeded")),
            Attribute::all(),
        );
        Ok(())
    }
}

/// Installs the shared accessors/methods of `IDBRequest`.
#[allow(clippy::too_many_lines)]
fn install_request_class(class: &mut ClassBuilder<'_>) -> JsResult<()> {
    let realm = class.context().realm().clone();

    let result_getter = NativeFunction::from_fn_ptr(|this, _args, context| {
        if let Some(obj) = this.as_object() {
            if let Some(state) = with_request_ref(&obj, |d| (d.ready_state, d.result.clone())) {
                let (ready_state, result) = state;
                if ready_state == ReadyState::Pending {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The request is still pending.",
                        context,
                    );
                }
                return Ok(result.unwrap_or(JsValue::undefined()));
            }
        }
        Ok(JsValue::undefined())
    })
    .to_js_function(&realm);

    let error_getter = NativeFunction::from_fn_ptr(|this, _args, context| {
        if let Some(obj) = this.as_object() {
            if let Some(state) = with_request_ref(&obj, |d| (d.ready_state, d.error.clone())) {
                let (ready_state, error) = state;
                if ready_state == ReadyState::Pending {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The request is still pending.",
                        context,
                    );
                }
                return Ok(error.unwrap_or(JsValue::null()));
            }
        }
        Ok(JsValue::null())
    })
    .to_js_function(&realm);

    let ready_state_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
        if let Some(obj) = this.as_object() {
            if let Some(state) = with_request_ref(&obj, |d| d.ready_state) {
                let state = match state {
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
            if let Some(source) = with_request_ref(&obj, |d| d.source.clone()).flatten() {
                return Ok(JsValue::from(source));
            }
        }
        Ok(JsValue::null())
    })
    .to_js_function(&realm);

    let transaction_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
        if let Some(obj) = this.as_object() {
            if let Some(txn) = with_request_ref(&obj, |d| {
                d.transaction.clone().or_else(|| d.upgrade_txn.clone())
            })
            .flatten()
            {
                return Ok(JsValue::from(txn));
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
        Some(handler_accessor(&realm, "success")),
        Some(handler_setter(&realm, "success")),
        Attribute::all(),
    );
    class.accessor(
        js_string!("onerror"),
        Some(handler_accessor(&realm, "error")),
        Some(handler_setter(&realm, "error")),
        Attribute::all(),
    );
    class.accessor(
        js_string!("oncomplete"),
        Some(handler_accessor(&realm, "complete")),
        Some(handler_setter(&realm, "complete")),
        Attribute::all(),
    );

    Ok(())
}

/// Builds a setter for an attribute handler (`onsuccess = fn`).
fn handler_setter(
    realm: &boa_engine::realm::Realm,
    event: &'static str,
) -> boa_engine::object::builtins::JsFunction {
    NativeFunction::from_copy_closure(move |this, args, _ctx| {
        let value = args.first().cloned().unwrap_or(JsValue::null());
        if let Some(obj) = this.as_object() {
            with_request_mut(&obj, |d| {
                d.set_handler(event, value);
            });
        }
        Ok(JsValue::undefined())
    })
    .to_js_function(realm)
}

/// Builds a getter for an attribute handler (`onsuccess`…).
fn handler_accessor(
    realm: &boa_engine::realm::Realm,
    event: &'static str,
) -> boa_engine::object::builtins::JsFunction {
    NativeFunction::from_copy_closure(move |this, _args, _ctx| {
        if let Some(obj) = this.as_object()
            && let Some(handler) = with_request_ref(&obj, |d| {
                d.attr_handlers
                    .borrow()
                    .get(&event.to_string())
                    .cloned()
                    .unwrap_or(JsValue::null())
            })
        {
            return Ok(handler);
        }
        Ok(JsValue::null())
    })
    .to_js_function(realm)
}

/// Installs `addEventListener` / `removeEventListener` on a request class.
fn install_event_listener_methods(class: &mut ClassBuilder<'_>) -> JsResult<()> {
    // addEventListener(type, callback)
    class.method(
        js_string!("addEventListener"),
        2,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let event_type = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();
            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
            if !callback.is_callable() {
                return Ok(JsValue::undefined());
            }
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBRequest>()
            {
                let mut listeners = data.listeners.borrow_mut();
                let id = listeners.len() as u64;
                listeners.push(crate::dom::event_target::EventListenerEntry {
                    event_type,
                    callback,
                    capture: false,
                    once: false,
                    passive: false,
                    id,
                });
            }
            Ok(JsValue::undefined())
        }),
    );

    // removeEventListener(type, callback)
    class.method(
        js_string!("removeEventListener"),
        2,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let event_type = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();
            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBRequest>()
            {
                let mut listeners = data.listeners.borrow_mut();
                listeners.retain(|l| {
                    !(l.event_type == event_type
                        && crate::dom::event_target::listener_equal(&l.callback, &callback))
                });
            }
            Ok(JsValue::undefined())
        }),
    );

    Ok(())
}
