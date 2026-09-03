//! `IDBFactory` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::compare::compare_keys;

use crate::convert::key::value_to_key;
use crate::runtime::IdbRuntime;

/// `IDBFactory` class — singleton on globalThis.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBFactory;

/// Checks that `this` is an IDBFactory.
fn check_factory_brand(this: &JsValue) -> JsResult<()> {
    let obj = this.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("IDBFactory method called on non-object")
    })?;
    if obj.downcast_ref::<IdBFactory>().is_some() {
        Ok(())
    } else {
        Err(JsNativeError::typ()
            .with_message("'this' is not an IDBFactory")
            .into())
    }
}

impl Class for IdBFactory {
    const NAME: &'static str = "IDBFactory";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBFactory cannot be constructed with new")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        // open(name, version) -> IDBOpenDBRequest
        class.method(
            js_string!("open"),
            2,
            NativeFunction::from_fn_ptr(|this, args, context| {
                check_factory_brand(this)?;

                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                let version = if let Some(v) = args.get(1) {
                    if v.is_undefined() {
                        1u64
                    } else {
                        crate::convert::webidl::to_unsigned_long_long_enforce_range(v, context)?
                    }
                } else {
                    1u64
                };

                if version == 0 {
                    return Err(JsNativeError::typ()
                        .with_message("The version provided must not be 0.")
                        .into());
                }

                // Get the runtime and open the database
                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let result = runtime.with_engine(|engine| engine.open_database(&name, version));

                match result {
                    Ok(open_result) => {
                        // Create an IDBOpenDBRequest with the result
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;

                        // Set result to the database connection
                        if let Some(mut data) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            data.ready_state = crate::api::request::ReadyState::Done;

                            // Create IDBDatabase object
                            let db_data = crate::api::database::IdBDatabase {
                                connection_id: open_result.connection_id,
                                name: name.clone(),
                                version: open_result.version,
                                closed: false,
                            };
                            let db_obj =
                                crate::api::database::IdBDatabase::from_data(db_data, context)?;
                            data.result = Some(JsValue::from(db_obj));
                        }

                        Ok(JsValue::from(req_obj))
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("Failed to open database: {e}"))
                        .into()),
                }
            }),
        );

        // deleteDatabase(name) -> IDBOpenDBRequest
        class.method(
            js_string!("deleteDatabase"),
            1,
            NativeFunction::from_fn_ptr(|this, args, context| {
                check_factory_brand(this)?;

                let name = args
                    .first()
                    .unwrap_or(&JsValue::undefined())
                    .to_string(context)?
                    .to_std_string_escaped();

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let result = runtime.with_engine(|engine| engine.delete_database(&name));

                match result {
                    Ok(()) => {
                        let req_data = crate::api::request::IdBRequest::new(0);
                        let req_obj =
                            crate::api::request::IdBRequest::from_data(req_data, context)?;
                        if let Some(mut data) =
                            req_obj.downcast_mut::<crate::api::request::IdBRequest>()
                        {
                            data.ready_state = crate::api::request::ReadyState::Done;
                            data.result = Some(JsValue::from(0u64)); // oldVersion = 0
                        }
                        Ok(JsValue::from(req_obj))
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("Failed to delete database: {e}"))
                        .into()),
                }
            }),
        );

        // databases() -> Promise<sequence<DatabaseInfo>>
        class.method(
            js_string!("databases"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                check_factory_brand(this)?;

                let runtime = context.get_data::<IdbRuntime>().ok_or_else(|| {
                    JsNativeError::error().with_message("IndexedDB not initialized")
                })?;

                let dbs = runtime.with_engine(|engine| engine.list_databases());

                match dbs {
                    Ok(list) => {
                        // Return as a JS array of {name, version} objects
                        let arr: Vec<JsValue> = list
                            .iter()
                            .map(|(name, version)| {
                                let obj = JsObject::with_null_proto();
                                obj.set(
                                    js_string!("name"),
                                    JsValue::from(js_string!(name.as_str())),
                                    false,
                                    context,
                                )?;
                                obj.set(
                                    js_string!("version"),
                                    JsValue::from(*version),
                                    false,
                                    context,
                                )?;
                                Ok(JsValue::from(obj))
                            })
                            .collect::<JsResult<Vec<_>>>()?;
                        Ok(JsValue::from(
                            boa_engine::object::builtins::JsArray::from_iter(arr, context),
                        ))
                    }
                    Err(e) => Err(JsNativeError::error()
                        .with_message(format!("Failed to list databases: {e}"))
                        .into()),
                }
            }),
        );

        // cmp(first, second) -> -1 | 0 | 1
        class.method(
            js_string!("cmp"),
            2,
            NativeFunction::from_fn_ptr(|this, args, context| {
                check_factory_brand(this)?;

                let default = JsValue::undefined();
                let first = args.first().unwrap_or(&default);
                let second = args.get(1).unwrap_or(&default);

                let k1 = value_to_key(first, context)
                    .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;
                let k2 = value_to_key(second, context)
                    .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;

                Ok(JsValue::from(compare_keys(&k1, &k2) as i32))
            }),
        );

        Ok(())
    }
}
