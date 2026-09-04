//! `IDBTransaction` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::proto::TxnMode;

use crate::dom::event_target::add_event_target_methods;
use crate::runtime::IdbRuntime;

/// Native data for `IDBTransaction`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBTransaction {
    #[unsafe_ignore_trace]
    pub txn_id: u64,
    #[unsafe_ignore_trace]
    pub mode: TxnMode,
    pub db: JsObject,
    pub active: bool,
    #[unsafe_ignore_trace]
    pub finished: bool,
    #[unsafe_ignore_trace]
    pub durability: boa_idb_core::proto::Durability,
    #[unsafe_ignore_trace]
    pub scope: Vec<u64>,
    /// Listeners registered through `addEventListener`.
    pub listeners: boa_gc::GcRefCell<Vec<crate::dom::event_target::EventListenerEntry>>,
    /// Attribute handlers (`oncomplete`, `onerror`, `onabort`).
    pub attr_handlers: boa_gc::GcRefCell<std::collections::HashMap<String, JsValue>>,
    /// Cached `IDBObjectStore` handles by name (one handle per store, §2.6.1).
    pub stores: boa_gc::GcRefCell<std::collections::HashMap<String, JsObject>>,
    /// Explicit `commit()` was called: new requests are rejected, queued
    /// requests still run to auto-commit.
    pub explicit_commit: bool,
}

impl IdBTransaction {
    pub fn new(txn_id: u64, mode: TxnMode, db: JsObject) -> Self {
        Self {
            txn_id,
            mode,
            db,
            active: true,
            finished: false,
            durability: boa_idb_core::proto::Durability::Default,
            scope: Vec::new(),
            listeners: boa_gc::GcRefCell::default(),
            attr_handlers: boa_gc::GcRefCell::default(),
            stores: boa_gc::GcRefCell::default(),
            explicit_commit: false,
        }
    }

    /// Sets an attribute handler (`oncomplete` → `complete`).
    pub fn set_handler(&self, name: &str, value: JsValue) {
        self.attr_handlers
            .borrow_mut()
            .insert(name.to_string(), value);
    }
}

impl Class for IdBTransaction {
    const NAME: &'static str = "IDBTransaction";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBTransaction cannot be constructed directly")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        add_event_target_methods(class)?;

        // objectStoreNames getter (scope names from the driver snapshot)
        let store_names_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBTransaction>() {
                    let names: Vec<String> = ctx
                        .get_data::<IdbRuntime>()
                        .map(|runtime| {
                            let d = crate::runtime::lock_mutex(&runtime.driver);
                            match d.txns.get(&data.txn_id) {
                                Some(handle) => handle
                                    .meta
                                    .stores
                                    .iter()
                                    .filter(|s| !s.deleted && data.scope.contains(&s.id))
                                    .map(|s| s.name.to_string())
                                    .collect(),
                                None => Vec::new(),
                            }
                        })
                        .unwrap_or_default();
                    return Ok(JsValue::from(crate::dom::string_list::dom_string_list(
                        names, ctx,
                    )?));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        // mode getter
        let mode_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBTransaction>() {
                    let mode_str = match data.mode {
                        TxnMode::ReadOnly => "readonly",
                        TxnMode::ReadWrite => "readwrite",
                        TxnMode::VersionChange => "versionchange",
                    };
                    return Ok(JsValue::from(js_string!(mode_str)));
                }
            }
            Ok(JsValue::from(js_string!("readonly")))
        })
        .to_js_function(&realm);

        // durability getter
        let durability_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBTransaction>() {
                    let dur_str = match data.durability {
                        boa_idb_core::proto::Durability::Default => "default",
                        boa_idb_core::proto::Durability::Strict => "strict",
                        boa_idb_core::proto::Durability::Relaxed => "relaxed",
                    };
                    return Ok(JsValue::from(js_string!(dur_str)));
                }
            }
            Ok(JsValue::from(js_string!("default")))
        })
        .to_js_function(&realm);

        // db getter
        let db_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBTransaction>() {
                    return Ok(JsValue::from(data.db.clone()));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        // error getter
        let error_getter = NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::null()))
            .to_js_function(&realm);

        class.accessor(
            js_string!("objectStoreNames"),
            Some(store_names_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("mode"),
            Some(mode_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("durability"),
            Some(durability_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(js_string!("db"), Some(db_getter), None, Attribute::READONLY);
        class.accessor(
            js_string!("error"),
            Some(error_getter),
            None,
            Attribute::READONLY,
        );

        // objectStore(name) -> IDBObjectStore (same handle per store, §2.6.1)
        class.method(
            js_string!("objectStore"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                })?;
                // Finished transactions reject with InvalidState; inactive
                // (but unfinished) ones still serve handles for queued work.
                let (txn_id, finished) = {
                    let data = obj.downcast_ref::<IdBTransaction>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                    })?;
                    (data.txn_id, data.finished)
                };
                if finished {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The transaction is finished.",
                        context,
                    );
                }

                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                // Same-handle cache first.
                if let Some(data) = obj.downcast_ref::<IdBTransaction>() {
                    if let Some(cached) = data.stores.borrow().get(&name).cloned() {
                        return Ok(JsValue::from(cached));
                    }
                }

                let Some(meta) = context.get_data::<IdbRuntime>().and_then(|runtime| {
                    let d = crate::runtime::lock_mutex(&runtime.driver);
                    crate::driver::txn_store_id(&d, txn_id, &name)
                        .ok()
                        .and_then(|sid| crate::driver::txn_store_view(&d, txn_id, sid).ok())
                }) else {
                    return crate::dom::exception::throw_not_found_error(
                        &format!("Object store '{name}' not found in transaction scope"),
                        context,
                    );
                };

                let store_obj = crate::api::object_store::IdBObjectStore::from_data(
                    crate::api::object_store::IdBObjectStore {
                        store_id: meta.id,
                        name: meta.name.to_string(),
                        key_path: Some(meta.key_path),
                        auto_increment: meta.auto_increment,
                        transaction: obj.clone(),
                        indexes: boa_gc::GcRefCell::default(),
                    },
                    context,
                )?;
                if let Some(data) = obj.downcast_ref::<IdBTransaction>() {
                    data.stores.borrow_mut().insert(name, store_obj.clone());
                }
                Ok(JsValue::from(store_obj))
            }),
        );

        // commit(): new requests are rejected from now on; queued work still
        // runs to auto-commit.
        class.method(
            js_string!("commit"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                })?;
                let (txn_id, active, finished, explicit) = {
                    let data = obj.downcast_ref::<IdBTransaction>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                    })?;
                    (
                        data.txn_id,
                        data.active,
                        data.finished,
                        data.explicit_commit,
                    )
                };
                // A transaction whose scope went inactive across a task
                // boundary (timer fire) cannot be committed (§2.7): compare
                // the task epochs, not just the sticky `active` flag, which
                // a `keep_alive` spin keeps set indefinitely.
                let epoch_stale = if let Some(runtime) = context.get_data::<IdbRuntime>() {
                    let d = crate::runtime::lock_mutex(&runtime.driver);
                    d.txns
                        .get(&txn_id)
                        .is_some_and(|h| h.active_epoch != d.task_epoch)
                } else {
                    false
                };
                if finished || explicit || !active || epoch_stale {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The transaction cannot be committed in its current state.",
                        context,
                    );
                }
                if let Some(mut data) = obj.downcast_mut::<IdBTransaction>() {
                    data.explicit_commit = true;
                }
                if let Some(runtime) = context.get_data::<IdbRuntime>() {
                    let mut d = crate::runtime::lock_mutex(&runtime.driver);
                    if let Some(handle) = d.txns.get_mut(&txn_id) {
                        handle.explicit_commit = true;
                    }
                }
                crate::runtime::schedule_pump(context);
                Ok(JsValue::undefined())
            }),
        );

        // abort(): rolls back immediately and fires `abort`.
        class.method(
            js_string!("abort"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                })?;
                let (txn_id, active, finished, explicit) = {
                    let data = obj.downcast_ref::<IdBTransaction>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                    })?;
                    (
                        data.txn_id,
                        data.active,
                        data.finished,
                        data.explicit_commit,
                    )
                };
                if finished || explicit || !active {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The transaction cannot be aborted in its current state.",
                        context,
                    );
                }
                if let Some(mut data) = obj.downcast_mut::<IdBTransaction>() {
                    data.active = false;
                    data.finished = true;
                }
                // Scoped handle: `abort_transaction` dispatches (reentrant),
                // so no guard may be held across the call.
                let driver_handle = context
                    .get_data::<IdbRuntime>()
                    .map(|runtime| runtime.driver.clone());
                if let Some(driver_handle) = driver_handle {
                    crate::driver::abort_transaction(&driver_handle, txn_id, context, true);
                }
                Ok(JsValue::undefined())
            }),
        );

        // oncomplete / onerror / onabort handlers (getter + setter pairs)
        let oncomplete_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBTransaction>()
            {
                return Ok(data
                    .attr_handlers
                    .borrow()
                    .get("complete")
                    .cloned()
                    .unwrap_or(JsValue::null()));
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);
        let set_oncomplete = NativeFunction::from_fn_ptr(|this, args, _ctx| {
            let value = args.first().cloned().unwrap_or(JsValue::null());
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBTransaction>()
            {
                data.set_handler("complete", value);
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);
        class.accessor(
            js_string!("oncomplete"),
            Some(oncomplete_getter),
            Some(set_oncomplete),
            Attribute::all(),
        );

        let onerror_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBTransaction>()
            {
                return Ok(data
                    .attr_handlers
                    .borrow()
                    .get("error")
                    .cloned()
                    .unwrap_or(JsValue::null()));
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);
        let set_onerror = NativeFunction::from_fn_ptr(|this, args, _ctx| {
            let value = args.first().cloned().unwrap_or(JsValue::null());
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBTransaction>()
            {
                data.set_handler("error", value);
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);
        class.accessor(
            js_string!("onerror"),
            Some(onerror_getter),
            Some(set_onerror),
            Attribute::all(),
        );

        let onabort_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBTransaction>()
            {
                return Ok(data
                    .attr_handlers
                    .borrow()
                    .get("abort")
                    .cloned()
                    .unwrap_or(JsValue::null()));
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);
        let set_onabort = NativeFunction::from_fn_ptr(|this, args, _ctx| {
            let value = args.first().cloned().unwrap_or(JsValue::null());
            if let Some(obj) = this.as_object()
                && let Some(data) = obj.downcast_ref::<IdBTransaction>()
            {
                data.set_handler("abort", value);
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);
        class.accessor(
            js_string!("onabort"),
            Some(onabort_getter),
            Some(set_onabort),
            Attribute::all(),
        );

        Ok(())
    }
}
