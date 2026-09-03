//! `IDBIndex` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::proto::SourceRef;

use crate::api::support::{active_txn_id, issue_request, parse_get_all_args, query_to_range};
use crate::driver::PendingOp;
use crate::runtime::IdbRuntime;

/// Native data for `IDBIndex` (also the class data).
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBIndexData {
    #[unsafe_ignore_trace]
    pub index_id: u64,
    #[unsafe_ignore_trace]
    pub name: String,
    #[unsafe_ignore_trace]
    pub key_path: KeyPath,
    #[unsafe_ignore_trace]
    pub unique: bool,
    #[unsafe_ignore_trace]
    pub multi_entry: bool,
    pub object_store: JsObject,
}

impl IdBIndexData {
    pub fn new(
        index_id: u64,
        name: String,
        key_path: KeyPath,
        unique: bool,
        multi_entry: bool,
        object_store: JsObject,
    ) -> Self {
        Self {
            index_id,
            name,
            key_path,
            unique,
            multi_entry,
            object_store,
        }
    }
}

/// Brand-checks `this` and splits off `(index_id, store_id, txn_obj)`.
fn index_of(this: &JsValue, context: &mut Context) -> JsResult<(JsObject, u64, u64, JsObject)> {
    let obj = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBIndex"))?;
    let (index_id, store_obj) = {
        let data = obj
            .downcast_ref::<IdBIndexData>()
            .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBIndex"))?;
        (data.index_id, data.object_store.clone())
    };
    let (store_id, txn_obj) = {
        let store_data = store_obj
            .downcast_ref::<crate::api::object_store::IdBObjectStore>()
            .ok_or_else(|| JsNativeError::typ().with_message("Index store is gone"))?;
        (store_data.store_id, store_data.transaction.clone())
    };
    let txn_id = active_txn_id(context, &txn_obj)?;
    Ok((obj, index_id, store_id, txn_obj))
}

impl Class for IdBIndexData {
    const NAME: &'static str = "IDBIndex";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBIndex cannot be constructed directly")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        let name_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBIndexData>() {
                    return Ok(JsValue::from(js_string!(data.name.as_str())));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);
        let set_name_fn = NativeFunction::from_fn_ptr(|this, args, context| {
            let obj = this
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBIndex"))?;
            let (index_id, store_name, txn_obj, old_name) = {
                let data = obj.downcast_ref::<IdBIndexData>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBIndex")
                })?;
                let store_data = data
                    .object_store
                    .downcast_ref::<crate::api::object_store::IdBObjectStore>()
                    .ok_or_else(|| JsNativeError::typ().with_message("Index store is gone"))?;
                (
                    data.index_id,
                    store_data.name.clone(),
                    store_data.transaction.clone(),
                    data.name.clone(),
                )
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
                crate::driver::schema_rename_index(
                    &mut d,
                    txn_id,
                    &store_name,
                    &old_name,
                    &new_name,
                )
            }
            .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;
            if let Some(mut data) = obj.downcast_mut::<IdBIndexData>() {
                data.name = new_name;
            }
            let _ = index_id;
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let unique_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBIndexData>() {
                    return Ok(JsValue::from(data.unique));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        let multi_entry_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBIndexData>() {
                    return Ok(JsValue::from(data.multi_entry));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        let object_store_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBIndexData>() {
                    return Ok(JsValue::from(data.object_store.clone()));
                }
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
        class.accessor(
            js_string!("unique"),
            Some(unique_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("multiEntry"),
            Some(multi_entry_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("objectStore"),
            Some(object_store_getter),
            None,
            Attribute::READONLY,
        );

        // keyPath getter (fresh Array per access for sequence paths)
        let key_path_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBIndexData>() {
                    return match &data.key_path {
                        KeyPath::Empty => Ok(JsValue::null()),
                        KeyPath::Single(s) => {
                            Ok(JsValue::from(boa_engine::JsString::from(s.as_slice())))
                        }
                        KeyPath::Array(paths) => {
                            let arr: Vec<JsValue> = paths
                                .iter()
                                .map(|s| JsValue::from(boa_engine::JsString::from(s.as_slice())))
                                .collect();
                            Ok(boa_engine::object::builtins::JsArray::from_iter(arr, ctx).into())
                        }
                    };
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

        // get(query)
        class.method(
            js_string!("get"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, index_id, store_id, txn_obj) = index_of(this, context)?;
                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let range = query_to_range(query, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::Get {
                        source: SourceRef::Index {
                            store: store_id,
                            index: index_id,
                        },
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
                let (obj, index_id, store_id, txn_obj) = index_of(this, context)?;
                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let range = query_to_range(query, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetKey {
                        source: SourceRef::Index {
                            store: store_id,
                            index: index_id,
                        },
                        range,
                    },
                    Some(obj),
                )?))
            }),
        );

        // getAll / getAllKeys / getAllRecords
        class.method(
            js_string!("getAll"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, index_id, store_id, txn_obj) = index_of(this, context)?;
                let parsed = crate::api::support::parse_get_all_args(args, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetAll {
                        source: SourceRef::Index {
                            store: store_id,
                            index: index_id,
                        },
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
                let (obj, index_id, store_id, txn_obj) = index_of(this, context)?;
                let parsed = crate::api::support::parse_get_all_args(args, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetAll {
                        source: SourceRef::Index {
                            store: store_id,
                            index: index_id,
                        },
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
                let (obj, index_id, store_id, txn_obj) = index_of(this, context)?;
                let parsed = crate::api::support::parse_get_all_args(args, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::GetAllRecords {
                        source: SourceRef::Index {
                            store: store_id,
                            index: index_id,
                        },
                        range: parsed.range,
                        limit: parsed.limit,
                        direction: parsed.direction,
                    },
                    Some(obj),
                )?))
            }),
        );

        // count(query?)
        class.method(
            js_string!("count"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let (obj, index_id, store_id, txn_obj) = index_of(this, context)?;
                let default_val = JsValue::undefined();
                let query = args.first().unwrap_or(&default_val);
                let range = query_to_range(query, context)?;
                let txn_id = active_txn_id(context, &txn_obj)?;
                Ok(JsValue::from(issue_request(
                    context,
                    txn_id,
                    PendingOp::Count {
                        source: SourceRef::Index {
                            store: store_id,
                            index: index_id,
                        },
                        range,
                    },
                    Some(obj),
                )?))
            }),
        );

        // openCursor(query?, direction?) / openKeyCursor(query?, direction?)
        class.method(
            js_string!("openCursor"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                open_cursor_impl(this, args, context, false)
            }),
        );
        class.method(
            js_string!("openKeyCursor"),
            0,
            NativeFunction::from_fn_ptr(|this, args, context| {
                open_cursor_impl(this, args, context, true)
            }),
        );

        Ok(())
    }
}

/// Shared index openCursor/openKeyCursor implementation.
fn open_cursor_impl(
    this: &JsValue,
    args: &[JsValue],
    context: &mut Context,
    key_only: bool,
) -> JsResult<JsValue> {
    let (obj, index_id, store_id, txn_obj) = index_of(this, context)?;
    let default_val = JsValue::undefined();
    let query = args.first().unwrap_or(&default_val);
    let range = query_to_range(query, context)?;
    let direction = match args.get(1) {
        Some(v) if !v.is_undefined() => crate::api::support::parse_direction(v, context)?,
        _ => boa_idb_core::proto::Direction::Next,
    };
    let txn_id = active_txn_id(context, &txn_obj)?;
    Ok(JsValue::from(issue_request(
        context,
        txn_id,
        PendingOp::OpenCursor {
            source: SourceRef::Index {
                store: store_id,
                index: index_id,
            },
            range,
            direction,
            key_only,
        },
        Some(obj),
    )?))
}
