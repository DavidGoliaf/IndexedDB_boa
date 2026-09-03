//! `IDBDatabase` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};

use crate::convert::webidl;
use crate::dom::event_target::add_event_target_methods;
use crate::runtime::IdbRuntime;

/// Native data for `IDBDatabase`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBDatabase {
    #[unsafe_ignore_trace]
    pub connection_id: u64,
    #[unsafe_ignore_trace]
    pub name: String,
    #[unsafe_ignore_trace]
    pub version: u64,
    pub closed: bool,
    /// Versionchange transaction id during an upgrade (if any).
    #[unsafe_ignore_trace]
    pub upgrade_txn_id: Option<u64>,
    /// Listeners registered through `addEventListener`.
    pub listeners: boa_gc::GcRefCell<Vec<crate::dom::event_target::EventListenerEntry>>,
    /// Attribute handlers (`onabort`, `onerror`, `onclose`, `onversionchange`).
    pub attr_handlers: boa_gc::GcRefCell<std::collections::HashMap<String, JsValue>>,
}

impl IdBDatabase {
    pub fn new(connection_id: u64, name: String, version: u64) -> Self {
        Self {
            connection_id,
            name,
            version,
            closed: false,
            upgrade_txn_id: None,
            listeners: boa_gc::GcRefCell::default(),
            attr_handlers: boa_gc::GcRefCell::default(),
        }
    }

    /// Sets an attribute handler (`onversionchange` → `versionchange`).
    pub fn set_handler(&self, name: &str, value: JsValue) {
        self.attr_handlers
            .borrow_mut()
            .insert(name.to_string(), value);
    }
}

impl Class for IdBDatabase {
    const NAME: &'static str = "IDBDatabase";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBDatabase cannot be constructed directly")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        add_event_target_methods(class)?;

        // name getter
        let name_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBDatabase>() {
                    return Ok(JsValue::from(js_string!(data.name.as_str())));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        // version getter
        let version_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBDatabase>() {
                    return Ok(JsValue::from(data.version));
                }
            }
            Ok(JsValue::from(0))
        })
        .to_js_function(&realm);

        // objectStoreNames getter (DOMStringList, code-unit sorted)
        let store_names_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBDatabase>() {
                    let (name, upgrade_txn) = (data.name.clone(), data.upgrade_txn_id);
                    let driver_handle = ctx
                        .get_data::<IdbRuntime>()
                        .map(|runtime| runtime.driver.clone());
                    let mut names =
                        crate::runtime::with_engine(ctx, |engine| match driver_handle {
                            Some(driver) => {
                                let guard = crate::runtime::lock_mutex(&driver);
                                crate::driver::db_store_names(engine, &guard, &name, upgrade_txn)
                            }
                            None => Vec::new(),
                        })?;
                    names.sort_by(|a, b| {
                        a.encode_utf16()
                            .collect::<Vec<_>>()
                            .cmp(&b.encode_utf16().collect::<Vec<_>>())
                    });
                    return Ok(JsValue::from(crate::dom::string_list::dom_string_list(
                        names, ctx,
                    )?));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("name"),
            Some(name_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("version"),
            Some(version_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("objectStoreNames"),
            Some(store_names_getter),
            None,
            Attribute::READONLY,
        );

        // createObjectStore(name, options) -> IDBObjectStore.
        //
        // Upgrade-only: the connection must carry a live versionchange
        // transaction. Executes synchronously against it (same atomic unit as
        // the queued upgrade work).
        class.method(
            js_string!("createObjectStore"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                let (upgrade_txn, closed) = {
                    let data = obj.downcast_ref::<IdBDatabase>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                    })?;
                    (data.upgrade_txn_id, data.closed)
                };
                if closed {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The database connection is closed.",
                        context,
                    );
                }

                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                // Parse options
                let mut key_path = None;
                let mut auto_increment = false;

                if let Some(opts) = args.get(1) {
                    if let Some(opts_obj) = opts.as_object() {
                        if let Ok(kp) = opts_obj.get(js_string!("keyPath"), context) {
                            key_path = webidl::to_key_path_argument(&kp, context)?;
                        }
                        if let Ok(ai) = opts_obj.get(js_string!("autoIncrement"), context) {
                            auto_increment = ai.to_boolean();
                        }
                    }
                }

                let Some(txn_id) = upgrade_txn else {
                    return crate::dom::exception::throw_invalid_state_error(
                        "createObjectStore requires a versionchange transaction.",
                        context,
                    );
                };

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;
                let meta = {
                    let mut d = crate::runtime::lock_mutex(&runtime.driver);
                    crate::driver::schema_create_store(
                        &mut d,
                        txn_id,
                        &name,
                        key_path,
                        auto_increment,
                    )
                }
                .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;

                let txn_obj = crate::runtime::txn_object(context, txn_id).ok_or_else(|| {
                    JsNativeError::error().with_message("Upgrade transaction is gone")
                })?;
                let store_obj = crate::api::object_store::IdBObjectStore::from_data(
                    crate::api::object_store::IdBObjectStore {
                        store_id: meta.id,
                        name: meta.name.to_string(),
                        key_path: Some(meta.key_path),
                        auto_increment: meta.auto_increment,
                        transaction: txn_obj.clone(),
                        indexes: boa_gc::GcRefCell::default(),
                    },
                    context,
                )?;
                // Same-handle cache (§2.6.1).
                if let Some(data) =
                    txn_obj.downcast_ref::<crate::api::transaction::IdBTransaction>()
                {
                    data.stores
                        .borrow_mut()
                        .insert(meta.name.to_string(), store_obj.clone());
                }
                Ok(JsValue::from(store_obj))
            }),
        );

        // deleteObjectStore(name)
        class.method(
            js_string!("deleteObjectStore"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                let (upgrade_txn, closed) = {
                    let data = obj.downcast_ref::<IdBDatabase>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                    })?;
                    (data.upgrade_txn_id, data.closed)
                };
                if closed {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The database connection is closed.",
                        context,
                    );
                }

                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                let Some(txn_id) = upgrade_txn else {
                    return crate::dom::exception::throw_invalid_state_error(
                        "deleteObjectStore requires a versionchange transaction.",
                        context,
                    );
                };

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;
                {
                    let mut d = crate::runtime::lock_mutex(&runtime.driver);
                    crate::driver::schema_delete_store(&mut d, txn_id, &name)
                }
                .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;
                // Drop the cached handle.
                if let Some(txn_obj) = crate::runtime::txn_object(context, txn_id) {
                    if let Some(data) =
                        txn_obj.downcast_ref::<crate::api::transaction::IdBTransaction>()
                    {
                        data.stores.borrow_mut().remove(&name);
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        // transaction(storeNames, mode?, options?) -> IDBTransaction
        class.method(
            js_string!("transaction"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                let (conn_id, db_name, closed, upgrade_txn) = {
                    let data = obj.downcast_ref::<IdBDatabase>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                    })?;
                    (
                        data.connection_id,
                        data.name.clone(),
                        data.closed,
                        data.upgrade_txn_id,
                    )
                };
                if closed {
                    return crate::dom::exception::throw_invalid_state_error(
                        "The database connection is closed.",
                        context,
                    );
                }

                // Parse store names (DOMString or sequence<DOMString>).
                let binding = JsValue::undefined();
                let store_names_val = args.first().unwrap_or(&binding);
                let store_names =
                    crate::convert::webidl::to_sequence_of_dom_strings(store_names_val, context)?
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();
                if store_names.is_empty() {
                    return Err(crate::dom::exception::throw_idb_error(
                        &boa_idb_core::error::IdbError::InvalidAccess(
                            "Transaction scope must not be empty.".into(),
                        ),
                        context,
                    ));
                }

                // Parse mode (invalid → TypeError).
                let mode = match args.get(1) {
                    None => boa_idb_core::proto::TxnMode::ReadOnly,
                    Some(m) if m.is_undefined() => boa_idb_core::proto::TxnMode::ReadOnly,
                    Some(m) => {
                        let mode_str = m.to_string(context)?.to_std_string_escaped();
                        match mode_str.as_str() {
                            "readonly" => boa_idb_core::proto::TxnMode::ReadOnly,
                            "readwrite" => boa_idb_core::proto::TxnMode::ReadWrite,
                            _ => {
                                return Err(JsNativeError::typ()
                                    .with_message(format!("Invalid transaction mode '{mode_str}'"))
                                    .into());
                            }
                        }
                    }
                };

                // Parse durability option.
                let mut durability = boa_idb_core::proto::Durability::Default;
                if let Some(opts) = args.get(2) {
                    if let Some(opts_obj) = opts.as_object() {
                        if let Ok(d) = opts_obj.get(js_string!("durability"), context) {
                            if !d.is_undefined() {
                                let s = d.to_string(context)?.to_std_string_escaped();
                                durability = match s.as_str() {
                                    "default" => boa_idb_core::proto::Durability::Default,
                                    "strict" => boa_idb_core::proto::Durability::Strict,
                                    "relaxed" => boa_idb_core::proto::Durability::Relaxed,
                                    _ => {
                                        return Err(JsNativeError::typ()
                                            .with_message(format!("Invalid durability '{s}'"))
                                            .into());
                                    }
                                };
                            }
                        }
                    }
                }

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                // A live upgrade transaction on this connection blocks normal ones.
                let blocked = {
                    let d = crate::runtime::lock_mutex(&runtime.driver);
                    match upgrade_txn {
                        Some(id) => d.txns.get(&id).is_some_and(|t| !t.finished),
                        None => false,
                    }
                };
                if blocked {
                    return crate::dom::exception::throw_invalid_state_error(
                        "An upgrade transaction is running on this connection.",
                        context,
                    );
                }

                // Resolve names to ids against committed metadata.
                // (Driver handle cloned first: the engine closure must not
                // capture borrows of `context`.)
                let driver_handle = context
                    .get_data::<IdbRuntime>()
                    .map(|runtime| runtime.driver.clone());
                let scope: Vec<u64> = crate::runtime::with_engine(context, |engine| {
                    let db = engine.open_db_handle(&db_name)?;
                    let meta = db.metadata().clone();
                    store_names
                        .iter()
                        .map(|name| {
                            let uname = boa_idb_core::key::utf16::Utf16String::from(name.as_str());
                            meta.stores
                                .iter()
                                .find(|s| !s.deleted && s.name == uname)
                                .map(|s| s.id)
                                .ok_or_else(|| {
                                    boa_idb_core::error::IdbError::NotFound(format!(
                                        "Object store '{name}' not found"
                                    ))
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })?
                .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;

                let txn_obj = crate::api::transaction::IdBTransaction::from_data(
                    crate::api::transaction::IdBTransaction::new(0, mode, obj.clone()),
                    context,
                )?;
                // Clone the driver handle first: the engine closure must not
                // capture `context` (already mutably borrowed by `with_engine`).
                let driver_handle = context
                    .get_data::<IdbRuntime>()
                    .map(|runtime| runtime.driver.clone());
                let txn_id = crate::runtime::with_engine(context, |engine| {
                    let Some(handle) = driver_handle else {
                        return Err(boa_idb_core::error::IdbError::Unknown(
                            "IndexedDB not initialized".into(),
                        ));
                    };
                    let mut d = crate::runtime::lock_mutex(&handle);
                    crate::driver::create_txn(
                        engine,
                        &mut d,
                        conn_id,
                        db_name.clone(),
                        mode,
                        scope.clone(),
                        durability,
                    )
                })?
                .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;
                if let Some(mut data) =
                    txn_obj.downcast_mut::<crate::api::transaction::IdBTransaction>()
                {
                    data.txn_id = txn_id;
                    data.mode = mode;
                    data.active = true;
                    data.finished = false;
                    data.scope = scope;
                }
                crate::runtime::register_txn(context, txn_id, txn_obj.clone());
                crate::runtime::schedule_pump(context);
                Ok(JsValue::from(txn_obj))
            }),
        );

        // close()
        class.method(
            js_string!("close"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                if let Some(obj) = this.as_object() {
                    let conn = obj
                        .downcast_ref::<IdBDatabase>()
                        .map(|data| data.connection_id);
                    if let Some(mut data) = obj.downcast_mut::<IdBDatabase>() {
                        data.closed = true;
                    }
                    if let Some(conn_id) = conn {
                        crate::driver::on_connection_closed(context, conn_id);
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        // onversionchange handler
        class.accessor(
            js_string!("onversionchange"),
            Some(
                NativeFunction::from_fn_ptr(|this, _args, _ctx| {
                    if let Some(obj) = this.as_object() {
                        if let Some(data) = obj.downcast_ref::<IdBDatabase>() {
                            return Ok(data
                                .attr_handlers
                                .borrow()
                                .get("versionchange")
                                .cloned()
                                .unwrap_or(JsValue::null()));
                        }
                    }
                    Ok(JsValue::null())
                })
                .to_js_function(&realm),
            ),
            Some(
                NativeFunction::from_fn_ptr(|this, args, _ctx| {
                    let value = args.first().cloned().unwrap_or(JsValue::null());
                    if let Some(obj) = this.as_object() {
                        if let Some(data) = obj.downcast_ref::<IdBDatabase>() {
                            data.set_handler("versionchange", value);
                        }
                    }
                    Ok(JsValue::undefined())
                })
                .to_js_function(&realm),
            ),
            Attribute::all(),
        );

        Ok(())
    }
}
