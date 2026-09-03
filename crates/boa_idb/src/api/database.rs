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
}

impl IdBDatabase {
    pub fn new(connection_id: u64, name: String, version: u64) -> Self {
        Self {
            connection_id,
            name,
            version,
            closed: false,
        }
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

        // objectStoreNames getter
        let store_names_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBDatabase>() {
                    let runtime = ctx.get_data::<IdbRuntime>().ok_or_else(|| {
                        JsNativeError::error().with_message("IndexedDB not initialized")
                    })?;
                    let names = runtime.with_engine(|engine| {
                        engine
                            .get_db_meta(&data.name)
                            .map(|meta| {
                                meta.stores
                                    .iter()
                                    .filter(|s| !s.deleted)
                                    .map(|s| JsValue::from(js_string!(s.name.to_string().as_str())))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    });
                    return Ok(JsValue::from(
                        boa_engine::object::builtins::JsArray::from_iter(names, ctx),
                    ));
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

        // createObjectStore(name, options) -> IDBObjectStore
        class.method(
            js_string!("createObjectStore"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                let data = obj.downcast_ref::<IdBDatabase>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                if data.closed {
                    return Err(JsNativeError::error()
                        .with_message("The database has been closed.")
                        .into());
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

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                // Need a versionchange transaction to create stores
                // For now, create directly via engine
                let result: Result<u64, boa_idb_core::error::IdbError> =
                    runtime.with_engine(|engine| {
                        // Find or create a versionchange transaction
                        let txn_id = engine.begin_transaction(
                            data.connection_id,
                            boa_idb_core::proto::TxnMode::VersionChange,
                            &[],
                            boa_idb_core::proto::Durability::Default,
                        )?;
                        let store_id =
                            engine.create_object_store(txn_id, &name, key_path, auto_increment)?;
                        engine.commit_transaction(txn_id)?;
                        Ok(store_id)
                    });

                match result {
                    Ok(_store_id) => {
                        // Return a placeholder IDBObjectStore
                        Err(JsNativeError::error()
                            .with_message(
                                "createObjectStore: IDBObjectStore return not yet implemented",
                            )
                            .into())
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("createObjectStore failed: {e}"))
                        .into()),
                }
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
                let data = obj.downcast_ref::<IdBDatabase>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                if data.closed {
                    return Err(JsNativeError::error()
                        .with_message("The database has been closed.")
                        .into());
                }

                let _name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?;

                // TODO: implement deleteObjectStore via engine
                Err(JsNativeError::error()
                    .with_message("deleteObjectStore() is not yet fully implemented")
                    .into())
            }),
        );

        // transaction(storeNames, mode, options) -> IDBTransaction
        class.method(
            js_string!("transaction"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                let obj = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                let data = obj.downcast_ref::<IdBDatabase>().ok_or_else(|| {
                    JsNativeError::typ().with_message("'this' is not an IDBDatabase")
                })?;
                if data.closed {
                    return Err(JsNativeError::error()
                        .with_message("The database has been closed.")
                        .into());
                }

                // Parse store names
                let binding = JsValue::undefined();
                let store_names_val = args.first().unwrap_or(&binding);
                let store_names: Vec<String> = if let Some(arr) = store_names_val.as_object() {
                    if arr.is_array() {
                        let length =
                            boa_engine::object::builtins::JsArray::from_object(arr.clone())?
                                .length(context)? as u32;
                        let mut names = Vec::with_capacity(length as usize);
                        for i in 0..length {
                            let elem = arr.get(i, context)?;
                            names.push(elem.to_string(context)?.to_std_string_escaped());
                        }
                        names
                    } else {
                        vec![store_names_val.to_string(context)?.to_std_string_escaped()]
                    }
                } else {
                    vec![store_names_val.to_string(context)?.to_std_string_escaped()]
                };

                // Parse mode
                let mode = if let Some(m) = args.get(1) {
                    let mode_str = m.to_string(context)?.to_std_string_escaped();
                    match mode_str.as_str() {
                        "readwrite" => boa_idb_core::proto::TxnMode::ReadWrite,
                        "readonly" => boa_idb_core::proto::TxnMode::ReadOnly,
                        "versionchange" => boa_idb_core::proto::TxnMode::VersionChange,
                        _ => boa_idb_core::proto::TxnMode::ReadOnly,
                    }
                } else {
                    boa_idb_core::proto::TxnMode::ReadOnly
                };

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let name_refs: Vec<&str> = store_names.iter().map(|s| s.as_str()).collect();
                let result = runtime.with_engine(|engine| {
                    engine.begin_transaction(
                        data.connection_id,
                        mode,
                        &name_refs,
                        boa_idb_core::proto::Durability::Default,
                    )
                });

                match result {
                    Ok(txn_id) => {
                        // Create IDBTransaction object
                        let txn_data = crate::api::transaction::IdBTransaction {
                            txn_id,
                            mode,
                            db: obj.clone(),
                            active: true,
                            durability: boa_idb_core::proto::Durability::Default,
                            scope: Vec::new(),
                        };
                        let txn_obj =
                            crate::api::transaction::IdBTransaction::from_data(txn_data, context)?;
                        Ok(JsValue::from(txn_obj))
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("transaction() failed: {e}"))
                        .into()),
                }
            }),
        );

        // close()
        class.method(
            js_string!("close"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                if let Some(obj) = this.as_object() {
                    if let Some(mut data) = obj.downcast_mut::<IdBDatabase>() {
                        if !data.closed {
                            data.closed = true;
                            let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                                JsNativeError::error().with_message("IndexedDB not initialized")
                            })?;
                            runtime.with_engine(|engine| {
                                engine.close_connection(data.connection_id);
                            });
                        }
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        // onversionchange handler
        class.accessor(
            js_string!("onversionchange"),
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
