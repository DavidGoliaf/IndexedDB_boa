//! `IDBObjectStore` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::path::KeyPath;

use crate::convert::key::value_to_key;
use crate::convert::value::{deserialize_from_storage, serialize_for_storage};
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
        }
    }
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

        // name getter/setter
        let name_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBObjectStore>() {
                    return Ok(JsValue::from(js_string!(data.name.as_str())));
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

        // keyPath getter
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

        // indexNames getter
        let index_names_getter = NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
            // TODO: return actual index names
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("indexNames"),
            Some(index_names_getter),
            None,
            Attribute::READONLY,
        );

        // put(value, key) -> IDBRequest
        class.method(
            js_string!("put"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;

                // Check transaction is active
                let txn_data = data
                    .transaction
                    .downcast_ref::<crate::api::transaction::IdBTransaction>()
                    .ok_or_else(|| {
                        JsNativeError::typ().with_message("Transaction data not found")
                    })?;
                if !txn_data.active {
                    return Err(JsNativeError::error()
                        .with_message("Transaction is not active")
                        .into());
                }

                let default_val = JsValue::undefined();
                let value_js = args.first().unwrap_or(&default_val);
                let explicit_key = args.get(1).filter(|k| !k.is_undefined());

                // Serialize value synchronously (AD-2)
                let mut sc_value = serialize_for_storage(value_js, context)?;

                // Convert explicit key if provided
                let key = explicit_key
                    .map(|k| value_to_key(k, context))
                    .transpose()
                    .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;

                // Execute via engine
                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let result = runtime.with_engine(|engine| {
                    engine.put(
                        txn_data.txn_id,
                        data.store_id,
                        &mut sc_value,
                        key.as_ref(),
                        false,
                    )
                });

                match result {
                    Ok(key) => {
                        // Create IDBRequest with result
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                            // Result is the key
                            req.result = Some(crate::convert::key::key_to_value(&key, context)?);
                        }
                        Ok(JsValue::from(req_obj))
                    }
                    Err(e) => {
                        // Create IDBRequest with error
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                            req.error = Some(crate::dom::exception::constraint_error(
                                &e.to_string(),
                                context,
                            )?);
                        }
                        Ok(JsValue::from(req_obj))
                    }
                }
            }),
        );

        // add(value, key) -> IDBRequest
        class.method(
            js_string!("add"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;

                let txn_data = data
                    .transaction
                    .downcast_ref::<crate::api::transaction::IdBTransaction>()
                    .ok_or_else(|| {
                        JsNativeError::typ().with_message("Transaction data not found")
                    })?;
                if !txn_data.active {
                    return Err(JsNativeError::error()
                        .with_message("Transaction is not active")
                        .into());
                }

                let default_val = JsValue::undefined();
                let value_js = args.first().unwrap_or(&default_val);
                let explicit_key = args.get(1).filter(|k| !k.is_undefined());

                let mut sc_value = serialize_for_storage(value_js, context)?;
                let key = explicit_key
                    .map(|k| value_to_key(k, context))
                    .transpose()
                    .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let result = runtime.with_engine(|engine| {
                    engine.put(
                        txn_data.txn_id,
                        data.store_id,
                        &mut sc_value,
                        key.as_ref(),
                        true,
                    )
                });

                match result {
                    Ok(key) => {
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                            req.result = Some(crate::convert::key::key_to_value(&key, context)?);
                        }
                        Ok(JsValue::from(req_obj))
                    }
                    Err(e) => {
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                            req.error = Some(crate::dom::exception::constraint_error(
                                &e.to_string(),
                                context,
                            )?);
                        }
                        Ok(JsValue::from(req_obj))
                    }
                }
            }),
        );

        // get(query) -> IDBRequest
        class.method(
            js_string!("get"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;

                let txn_data = data
                    .transaction
                    .downcast_ref::<crate::api::transaction::IdBTransaction>()
                    .ok_or_else(|| {
                        JsNativeError::typ().with_message("Transaction data not found")
                    })?;

                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let key = value_to_key(query, context)
                    .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let result = runtime.with_engine(|engine| {
                    engine.get(
                        txn_data.txn_id,
                        boa_idb_core::proto::SourceRef::Store(data.store_id),
                        &key,
                    )
                });

                match result {
                    Ok(Some(sc_value)) => {
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                            req.result = Some(deserialize_from_storage(&sc_value, context)?);
                        }
                        Ok(JsValue::from(req_obj))
                    }
                    Ok(None) => {
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                            req.result = Some(JsValue::undefined());
                        }
                        Ok(JsValue::from(req_obj))
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("get() failed: {e}"))
                        .into()),
                }
            }),
        );

        // getKey(query) -> IDBRequest
        class.method(
            js_string!("getKey"),
            1,
            NativeFunction::from_fn_ptr(|_this, args, context| {
                let _query = args.first().unwrap_or(&JsValue::undefined());
                Err(JsNativeError::error()
                    .with_message("getKey() is not yet fully implemented")
                    .into())
            }),
        );

        // getAll(query, count) -> IDBRequest
        class.method(
            js_string!("getAll"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("getAll() is not yet fully implemented")
                    .into())
            }),
        );

        // delete(query) -> IDBRequest
        class.method(
            js_string!("delete"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;

                let txn_data = data
                    .transaction
                    .downcast_ref::<crate::api::transaction::IdBTransaction>()
                    .ok_or_else(|| {
                        JsNativeError::typ().with_message("Transaction data not found")
                    })?;
                if !txn_data.active {
                    return Err(JsNativeError::error()
                        .with_message("Transaction is not active")
                        .into());
                }

                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let key = value_to_key(query, context)
                    .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let result = runtime
                    .with_engine(|engine| engine.delete(txn_data.txn_id, data.store_id, &key));

                match result {
                    Ok(_existed) => {
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                        }
                        Ok(JsValue::from(req_obj))
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("delete() failed: {e}"))
                        .into()),
                }
            }),
        );

        // clear() -> IDBRequest
        class.method(
            js_string!("clear"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;
                let data = obj.downcast_ref::<IdBObjectStore>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBObjectStore")
                })?;

                let txn_data = data
                    .transaction
                    .downcast_ref::<crate::api::transaction::IdBTransaction>()
                    .ok_or_else(|| {
                        JsNativeError::typ().with_message("Transaction data not found")
                    })?;
                if !txn_data.active {
                    return Err(JsNativeError::error()
                        .with_message("Transaction is not active")
                        .into());
                }

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let result =
                    runtime.with_engine(|engine| engine.clear(txn_data.txn_id, data.store_id));

                match result {
                    Ok(()) => {
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut req) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            req.ready_state = crate::api::request::ReadyState::Done;
                        }
                        Ok(JsValue::from(req_obj))
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("clear() failed: {e}"))
                        .into()),
                }
            }),
        );

        // count(query) -> IDBRequest
        class.method(
            js_string!("count"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("count() is not yet fully implemented")
                    .into())
            }),
        );

        // openCursor(query, direction) -> IDBRequest
        class.method(
            js_string!("openCursor"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("openCursor() is not yet fully implemented")
                    .into())
            }),
        );

        // openKeyCursor(query, direction) -> IDBRequest
        class.method(
            js_string!("openKeyCursor"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("openKeyCursor() is not yet fully implemented")
                    .into())
            }),
        );

        // index(name) -> IDBIndex
        class.method(
            js_string!("index"),
            1,
            NativeFunction::from_fn_ptr(|_this, args, context| {
                let _name = args.first().unwrap_or(&JsValue::undefined());
                Err(JsNativeError::error()
                    .with_message("index() is not yet fully implemented")
                    .into())
            }),
        );

        // createIndex(name, keyPath, options) -> IDBIndex
        class.method(
            js_string!("createIndex"),
            2,
            NativeFunction::from_fn_ptr(|_this, args, context| {
                let _name = args.first().unwrap_or(&JsValue::undefined());
                let _key_path = args.get(1);
                let _options = args.get(2);
                Err(JsNativeError::error()
                    .with_message("createIndex() is not yet fully implemented")
                    .into())
            }),
        );

        // deleteIndex(name)
        class.method(
            js_string!("deleteIndex"),
            1,
            NativeFunction::from_fn_ptr(|_this, args, context| {
                let _name = args.first().unwrap_or(&JsValue::undefined());
                Err(JsNativeError::error()
                    .with_message("deleteIndex() is not yet fully implemented")
                    .into())
            }),
        );

        Ok(())
    }
}
