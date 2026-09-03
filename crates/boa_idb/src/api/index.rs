//! `IDBIndex` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::path::KeyPath;

/// Native data for `IDBIndex`.
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

/// `IDBIndex` class.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBIndex;

impl Class for IdBIndex {
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
            None,
            Attribute::READONLY,
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

        // keyPath getter
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

        // get(query) -> IDBRequest
        class.method(
            js_string!("get"),
            1,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("IDBIndex.get() not yet implemented")
                    .into())
            }),
        );

        // getKey(query) -> IDBRequest
        class.method(
            js_string!("getKey"),
            1,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("IDBIndex.getKey() not yet implemented")
                    .into())
            }),
        );

        // getAll(query, count) -> IDBRequest
        class.method(
            js_string!("getAll"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("IDBIndex.getAll() not yet implemented")
                    .into())
            }),
        );

        // getAllKeys(query, count) -> IDBRequest
        class.method(
            js_string!("getAllKeys"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("IDBIndex.getAllKeys() not yet implemented")
                    .into())
            }),
        );

        // count(query) -> IDBRequest
        class.method(
            js_string!("count"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("IDBIndex.count() not yet implemented")
                    .into())
            }),
        );

        // openCursor(query, direction) -> IDBRequest
        class.method(
            js_string!("openCursor"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("IDBIndex.openCursor() not yet implemented")
                    .into())
            }),
        );

        // openKeyCursor(query, direction) -> IDBRequest
        class.method(
            js_string!("openKeyCursor"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("IDBIndex.openKeyCursor() not yet implemented")
                    .into())
            }),
        );

        Ok(())
    }
}
