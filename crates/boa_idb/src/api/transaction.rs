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
    pub durability: boa_idb_core::proto::Durability,
    #[unsafe_ignore_trace]
    pub scope: Vec<u64>,
}

impl IdBTransaction {
    pub fn new(txn_id: u64, mode: TxnMode, db: JsObject) -> Self {
        Self {
            txn_id,
            mode,
            db,
            active: true,
            durability: boa_idb_core::proto::Durability::Default,
            scope: Vec::new(),
        }
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

        // objectStoreNames getter
        let store_names_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBTransaction>() {
                    let runtime = ctx.get_data::<IdbRuntime>().ok_or_else(|| {
                        JsNativeError::error().with_message("IndexedDB not initialized")
                    })?;
                    let names: Vec<JsValue> = runtime.with_engine(|engine| {
                        data.scope
                            .iter()
                            .filter_map(|store_id| engine.get_store_meta("", *store_id))
                            .map(|s| JsValue::from(js_string!(s.name.to_string().as_str())))
                            .collect()
                    });
                    return Ok(JsValue::from(
                        boa_engine::object::builtins::JsArray::from_iter(names, ctx),
                    ));
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

        // objectStore(name) -> IDBObjectStore
        class.method(
            js_string!("objectStore"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                })?;
                let data = obj.downcast_ref::<IdBTransaction>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBTransaction")
                })?;
                if !data.active {
                    return Err(JsNativeError::error()
                        .with_message("Transaction is not active")
                        .into());
                }

                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                // Look up store by name
                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let store_meta = runtime.with_engine(|engine| {
                    // Find the store in the transaction scope
                    data.scope.iter().find_map(|store_id| {
                        engine.get_store_meta("", *store_id).and_then(|s| {
                            if s.name.to_string() == name {
                                Some(s.clone())
                            } else {
                                None
                            }
                        })
                    })
                });

                match store_meta {
                    Some(meta) => {
                        let store_data = crate::api::object_store::IdBObjectStore {
                            store_id: meta.id,
                            name: meta.name.to_string(),
                            key_path: Some(meta.key_path),
                            auto_increment: meta.auto_increment,
                            transaction: obj.clone(),
                        };
                        let store_obj = crate::api::object_store::IdBObjectStore::from_data(
                            store_data, context,
                        )?;
                        Ok(JsValue::from(store_obj))
                    }
                    None => Err(JsNativeError::error()
                        .with_message(format!(
                            "Object store '{name}' not found in transaction scope"
                        ))
                        .into()),
                }
            }),
        );

        // commit()
        class.method(
            js_string!("commit"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                if let Some(obj) = this.as_object() {
                    if let Some(mut data) = obj.downcast_mut::<IdBTransaction>() {
                        data.active = false;
                        let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                            JsNativeError::error().with_message("IndexedDB not initialized")
                        })?;
                        runtime.with_engine(|engine| {
                            let _ = engine.commit_transaction(data.txn_id);
                        });
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        // abort()
        class.method(
            js_string!("abort"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                if let Some(obj) = this.as_object() {
                    if let Some(mut data) = obj.downcast_mut::<IdBTransaction>() {
                        data.active = false;
                        let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                            JsNativeError::error().with_message("IndexedDB not initialized")
                        })?;
                        runtime.with_engine(|engine| {
                            let _ = engine.abort_transaction(data.txn_id);
                        });
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        // oncomplete / onerror / onabort handlers
        for event_name in &["oncomplete", "onerror", "onabort"] {
            class.accessor(
                js_string!(*event_name),
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
        }

        Ok(())
    }
}
