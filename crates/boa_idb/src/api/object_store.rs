//! `IDBObjectStore` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::proto::SourceRef;

use crate::api::support::{active_txn_id, issue_request, parse_get_all_args, query_to_range};
use crate::convert::key::value_to_key;
use crate::convert::value::serialize_for_storage;
use crate::driver::PendingOp;
use crate::runtime::IdbRuntime;

/// Native data for `IDBObjectStore`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBObjectStore {
    #[unsafe_ignore_trace]
    pub store_id: u64,
    #[unsafe_ignore_trace]
    pub name: String,
    #[unsafe_ignore_trace]
    pub key_path: Option<KeyPath>,
    #[unsafe_ignore_trace]
    pub auto_increment: bool,
    pub transaction: JsObject,
    /// Cached `IDBIndex` handles by name (one handle per index, §2.6.1).
    pub indexes: boa_gc::GcRefCell<std::collections::HashMap<String, JsObject>>,
}

impl IdBObjectStore {
    pub fn new(
        store_id: u64,
        name: String,
        key_path: Option<KeyPath>,
        auto_increment: bool,
        transaction: JsObject,
    ) -> Self {
        Self {
            store_id,
            name,
            key_path,
            auto_increment,
            transaction,
            indexes: boa_gc::GcRefCell::default(),
        }
    }
}

/// Brand-checks `this` and splits off owned store data.
fn store_of(this: &JsValue, context: &mut Context) -> JsResult<(JsObject, u64, JsObject)> {
    let obj = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBObjectStore"))?;
    let (store_id, txn_obj) = {
        let data = obj
            .downcast_ref::<IdBObjectStore>()
            .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBObjectStore"))?;
        (data.store_id, data.transaction.clone())
    };
    // Deleted/aborted-upgrade handles first (InvalidStateError), then the
    // activity check (TransactionInactiveError).
    crate::api::support::require_live_store(context, &txn_obj, store_id)?;
    let txn_id = active_txn_id(context, &txn_obj)?;
    Ok((obj, store_id, txn_obj))
}

/// Requires a readwrite transaction; throws `ReadOnlyError` otherwise.
fn require_readwrite(context: &mut Context, txn_obj: &JsObject) -> JsResult<()> {
    let readonly = txn_obj
        .downcast_ref::<crate::api::transaction::IdBTransaction>()
        .is_some_and(|d| d.mode == boa_idb_core::proto::TxnMode::ReadOnly);
    if readonly {
        return Err(crate::dom::exception::throw_idb_error(
            &boa_idb_core::error::IdbError::ReadOnly,
            context,
        ));
    }
    Ok(())
}

/// Shared openCursor/openKeyCursor implementation.
fn open_cursor_impl(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
    key_only: bool,
) -> JsResult<JsValue> {
    let (obj, store_id, txn_obj) = store_of(this, context)?;
    let default_val = JsValue::undefined();
    let query = args.first().unwrap_or(&default_val);
    let range = crate::api::support::nullable_query_to_range(query, context)?;
    let direction = match args.get(1) {
        Some(v) if !v.is_undefined() => crate::api::support::parse_direction(v, context)?,
        _ => boa_idb_core::proto::Direction::Next,
    };
    let txn_id = active_txn_id(context, &txn_obj)?;
    Ok(JsValue::from(issue_request(
        context,
        txn_id,
        PendingOp::OpenCursor {
            source: SourceRef::Store(store_id),
            range,
            direction,
            key_only,
        },
        Some(obj),
    )?))
}

/// Shared put/add implementation.
fn put_impl(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
    no_overwrite: bool,
) -> JsResult<JsValue> {
    let (obj, store_id, txn_obj) = store_of(this, context)?;
    require_readwrite(context, &txn_obj)?;

    let default_val = JsValue::undefined();
    let value_js = args.first().unwrap_or(&default_val);
    let explicit_key = args.get(1).filter(|k| !k.is_undefined());

    // Structured clone runs synchronously (AD-2); DataCloneError throws sync.
    let sc_value = serialize_for_storage(value_js, context).map_err(|e| {
        if e.as_opaque().is_some() {
            e
        } else {
            crate::dom::exception::throw_idb_error(
                &boa_idb_core::error::IdbError::DataClone(e.to_string()),
                context,
            )
        }
    })?;

    let key = explicit_key
        .map(|k| value_to_key(k, context))
        .transpose()
        .map_err(|e| crate::convert::key::throw_key_conversion_error(e, context))?;

    // Eager key validation (§6.1): invalid keys throw `DataError`
    // synchronously instead of surfacing through the request.
    {
        let (key_path, auto_increment) = {
            let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
            })?;
            (data.key_path.clone(), data.auto_increment)
        };
        let mut data_error = |msg: &str| {
            crate::dom::exception::throw_idb_error(
                &boa_idb_core::error::IdbError::Data(msg.into()),
                context,
            )
        };
        match (&key_path, &key) {
            // Explicit key with an inline-key store.
            (Some(kp), Some(_)) if !matches!(kp, KeyPath::Empty) => {
                return Err(data_error(
                    "Cannot provide an explicit key when the store has a keyPath",
                ));
            }
            // Inline-key store without explicit key: the key must be
            // extractable now, or generatable via autoIncrement.
            (Some(kp), None) if !matches!(kp, KeyPath::Empty) => match kp.extract(&sc_value) {
                Ok(Some(_)) => {}
                Ok(None) if auto_increment => {}
                Ok(None) => {
                    return Err(data_error(
                        "Could not extract a key from the value and autoIncrement is disabled",
                    ));
                }
                Err(e) => {
                    return Err(data_error(&format!("Invalid inline key: {e}")));
                }
            },
            // Empty-string key path without explicit key: the value itself is
            // the key and must already be a valid key (autoIncrement falls
            // back to generation, validated at execution).
            (Some(KeyPath::Empty), None) => match sc_value.to_key() {
                Ok(Some(_)) => {}
                _ if auto_increment => {}
                Ok(None) => {
                    return Err(data_error("Value cannot be used as a key"));
                }
                Err(e) => {
                    return Err(data_error(&format!("Invalid key: {e}")));
                }
            },
            // Out-of-line store without key and without autoIncrement.
            (kp, None)
                if kp.as_ref().is_none_or(|k| matches!(k, KeyPath::Empty)) && !auto_increment =>
            {
                return Err(data_error("No key provided and autoIncrement is disabled"));
            }
            _ => {}
        }
    }

    let txn_id = active_txn_id(context, &txn_obj)?;
    Ok(JsValue::from(issue_request(
        context,
        txn_id,
        PendingOp::Put {
            store_id,
            value: sc_value,
            explicit_key: key,
            no_overwrite,
        },
        Some(obj),
    )?))
}

impl Class for IdBObjectStore {
    const NAME: &'static str = "IDBObjectStore";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBObjectStore cannot be constructed directly")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        // name getter/setter (rename only in a live upgrade transaction)
        let name_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    return Ok(JsValue::from(js_string!(data.name.as_str())));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);
        let set_name_fn = NativeFunction::from_fn_ptr(|this, args, context| {
            let obj = this.as_object().ok_or_else(|| {
                JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
            })?;
            let (_store_id, txn_obj, old_name) = {
                let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                (data.store_id, data.transaction.clone(), data.name.clone())
            };
            let new_name = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();
            if new_name == old_name {
                return Ok(JsValue::undefined());
            }
            let txn_id = active_txn_id(context, &txn_obj)?;
            // Upgrade-only: the driver validates versionchange mode.
            let driver_handle = context
                .get_data::<IdbRuntime>()
                .map(|runtime| runtime.driver.clone());
            let Some(driver_handle) = driver_handle else {
                return Err(JsNativeError::error()
                    .with_message("IndexedDB not initialized")
                    .into());
            };
            {
                let mut d = crate::runtime::lock_mutex(&driver_handle);
                crate::driver::schema_rename_store(&mut d, txn_id, &old_name, &new_name)
            }
            .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;
            if let Some(mut data) = obj.downcast_mut::<IdBObjectStore>() {
                data.name.clone_from(&new_name);
            }
            // The cached handle keeps identity; re-key under the new name.
            if let Some(txn_data) =
                txn_obj.downcast_ref::<crate::api::transaction::IdBTransaction>()
            {
                let mut stores = txn_data.stores.borrow_mut();
                stores.remove(&old_name);
                stores.insert(new_name, obj.clone());
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("name"),
            Some(name_getter),
            Some(set_name_fn),
            Attribute::all(),
        );

        // keyPath getter (fresh Array per access for sequence paths)
        let key_path_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    if let Some(ref kp) = data.key_path {
                        return match kp {
                            KeyPath::Empty => Ok(JsValue::null()),
                            KeyPath::Single(s) => {
                                Ok(JsValue::from(boa_engine::JsString::from(s.as_slice())))
                            }
                            KeyPath::Array(paths) => {
                                let arr: Vec<JsValue> = paths
                                    .iter()
                                    .map(|s| {
                                        JsValue::from(boa_engine::JsString::from(s.as_slice()))
                                    })
                                    .collect();
                                Ok(boa_engine::object::builtins::JsArray::from_iter(arr, ctx)
                                    .into())
                            }
                        };
                    }
                    return Ok(JsValue::null());
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("keyPath"),
            Some(key_path_getter),
            None,
            Attribute::READONLY,
        );

        // autoIncrement getter
        let auto_increment_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    return Ok(JsValue::from(data.auto_increment));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("autoIncrement"),
            Some(auto_increment_getter),
            None,
            Attribute::READONLY,
        );

        // transaction getter
        let transaction_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    return Ok(JsValue::from(data.transaction.clone()));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("transaction"),
            Some(transaction_getter),
            None,
            Attribute::READONLY,
        );

        // indexNames getter (DOMStringList)
        let index_names_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    let (txn_id, store_id) = (data.transaction.clone(), data.store_id);
                    let txn_id = active_txn_id(ctx, &txn_id).unwrap_or(u64::MAX);
                    let names: Vec<String> = ctx
                        .get_data::<IdbRuntime>()
                        .map(|runtime| {
                            let d = crate::runtime::lock_mutex(&runtime.driver);
                            crate::driver::store_index_names(&d, txn_id, store_id)
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

        class.accessor(
            js_string!("indexNames"),
            Some(index_names_getter),
            None,
            Attribute::READONLY,
        );

        // put(value, key?) / add(value, key?)
        class.method(
            js_string!("put"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| put_impl(this, args, context, false)),
        );
        class.method(
            js_string!("add"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| put_impl(this, args, context, true)),
        );

        // get(query)
        class.method(
            js_string!("get"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let range = query_to_range(query, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::Get {
                        source: SourceRef::Store(store_id),
                        range,
                    },
                    Some(obj),
                )?))
            }),
        );

        // getKey(query)
        class.method(
            js_string!("getKey"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let range = query_to_range(query, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetKey {
                        source: SourceRef::Store(store_id),
                        range,
                    },
                    Some(obj),
                )?))
            }),
        );

        // getAll(query?, count?) / getAllKeys / getAllRecords
        class.method(
            js_string!("getAll"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                let parsed = crate::api::support::parse_get_all_args(args, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetAll {
                        source: SourceRef::Store(store_id),
                        range: parsed.range,
                        limit: parsed.limit,
                        keys_only: false,
                    },
                    Some(obj),
                )?))
            }),
        );
        class.method(
            js_string!("getAllKeys"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                let parsed = crate::api::support::parse_get_all_args(args, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetAll {
                        source: SourceRef::Store(store_id),
                        range: parsed.range,
                        limit: parsed.limit,
                        keys_only: true,
                    },
                    Some(obj),
                )?))
            }),
        );
        class.method(
            js_string!("getAllRecords"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                let parsed = crate::api::support::parse_get_all_args(args, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetAllRecords {
                        source: SourceRef::Store(store_id),
                        range: parsed.range,
                        limit: parsed.limit,
                        direction: parsed.direction,
                    },
                    Some(obj),
                )?))
            }),
        );

        // delete(query)
        class.method(
            js_string!("delete"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                require_readwrite(context, &txn_obj)?;
                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let range = query_to_range(query, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::Delete { store_id, range },
                    Some(obj),
                )?))
            }),
        );

        // clear()
        class.method(
            js_string!("clear"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                require_readwrite(context, &txn_obj)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::Clear { store_id },
                    Some(obj),
                )?))
            }),
        );

        // count(query?)
        class.method(
            js_string!("count"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, store_id, txn_obj) = store_of(this, context)?;
                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let range = query_to_range(query, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::Count {
                        source: SourceRef::Store(store_id),
                        range,
                    },
                    Some(obj),
                )?))
            }),
        );

        // openCursor(query?, direction?)
        class.method(
            js_string!("openCursor"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                open_cursor_impl(this, args, context, false)
            }),
        );

        // openKeyCursor(query?, direction?)
        class.method(
            js_string!("openKeyCursor"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                open_cursor_impl(this, args, context, true)
            }),
        );

        // index(name) -> IDBIndex (same handle per index, §2.6.1)
        class.method(
            js_string!("index"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let (store_id, txn_obj) = {
                    let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                    })?;
                    (data.store_id, data.transaction.clone())
                };
                crate::api::support::require_live_store(context, &txn_obj, store_id)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    if let Some(cached) = data.indexes.borrow().get(&name).cloned() {
                        return Ok(JsValue::from(cached));
                    }
                }

                let meta = match context.get_data::<IdbRuntime>() {
                    Some(runtime) => {
                        let d = crate::runtime::lock_mutex(&runtime.driver);
                        crate::driver::store_index_view(&d, txn_id, store_id, &name).ok()
                    }
                    None => None,
                };
                let Some(meta) = meta else {
                    return Err(crate::dom::exception::throw_idb_error(
                        &boa_idb_core::error::IdbError::NotFound(format!(
                            "Index '{name}' not found"
                        )),
                        context,
                    ));
                };
                let index_obj = crate::api::index::IdBIndexData::from_data(
                    crate::api::index::IdBIndexData::new(
                        meta.id,
                        meta.name.to_string(),
                        meta.key_path,
                        meta.unique,
                        meta.multi_entry,
                        obj.clone(),
                    ),
                    context,
                )?;
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    data.indexes.borrow_mut().insert(name, index_obj.clone());
                }
                Ok(JsValue::from(index_obj))
            }),
        );

        // createIndex(name, keyPath, options?)
        class.method(
            js_string!("createIndex"),
            2,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let (store_name, txn_obj) = {
                    let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                    })?;
                    (data.name.clone(), data.transaction.clone())
                };
                let txn_id = active_txn_id(context, &txn_obj)?;

                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();
                let kp_val = args.get(1).cloned().unwrap_or(JsValue::undefined());
                let key_path = crate::convert::webidl::to_key_path_argument(&kp_val, context)?;
                let Some(key_path) = key_path else {
                    return Err(JsNativeError::typ()
                        .with_message("createIndex requires a keyPath")
                        .into());
                };
                let (unique, multi_entry) = match args.get(2) {
                    Some(opts) => match opts.as_object() {
                        Some(opts_obj) => {
                            let unique = opts_obj
                                .get(js_string!("unique"), context)
                                .map(|v| v.to_boolean())
                                .unwrap_or(false);
                            let multi_entry = opts_obj
                                .get(js_string!("multiEntry"), context)
                                .map(|v| v.to_boolean())
                                .unwrap_or(false);
                            (unique, multi_entry)
                        }
                        None => (false, false),
                    },
                    None => (false, false),
                };

                let driver_handle = context
                    .get_data::<IdbRuntime>()
                    .map(|runtime| runtime.driver.clone());
                let meta = match driver_handle {
                    Some(handle) => {
                        let mut d = crate::runtime::lock_mutex(&handle);
                        crate::driver::schema_create_index(
                            &mut d,
                            txn_id,
                            &store_name,
                            &name,
                            key_path,
                            unique,
                            multi_entry,
                        )
                    }
                    None => Err(boa_idb_core::error::IdbError::Unknown(
                        "IndexedDB not initialized".into(),
                    )),
                };
                let meta = match meta {
                    Ok(meta) => meta,
                    Err(e) => {
                        return Err(crate::dom::exception::throw_idb_error(&e, context));
                    }
                };
                let index_obj = crate::api::index::IdBIndexData::from_data(
                    crate::api::index::IdBIndexData::new(
                        meta.id,
                        meta.name.to_string(),
                        meta.key_path,
                        meta.unique,
                        meta.multi_entry,
                        obj.clone(),
                    ),
                    context,
                )?;
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    data.indexes.borrow_mut().insert(name, index_obj.clone());
                }
                Ok(JsValue::from(index_obj))
            }),
        );

        // deleteIndex(name)
        class.method(
            js_string!("deleteIndex"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let (store_name, txn_obj) = {
                    let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                        JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                    })?;
                    (data.name.clone(), data.transaction.clone())
                };
                let txn_id = active_txn_id(context, &txn_obj)?;
                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                let driver_handle = context
                    .get_data::<IdbRuntime>()
                    .map(|runtime| runtime.driver.clone());
                let result = driver_handle.map(|handle| {
                    let mut d = crate::runtime::lock_mutex(&handle);
                    crate::driver::schema_delete_index(&mut d, txn_id, &store_name, &name)
                });
                match result {
                    Some(Ok(())) => {
                        if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                            data.indexes.borrow_mut().remove(&name);
                        }
                        Ok(JsValue::undefined())
                    }
                    Some(Err(e)) => Err(crate::dom::exception::throw_idb_error(&e, context)),
                    None => Err(JsNativeError::error()
                        .with_message("IndexedDB not initialized")
                        .into()),
                }
            }),
        );

        Ok(())
    }
}
