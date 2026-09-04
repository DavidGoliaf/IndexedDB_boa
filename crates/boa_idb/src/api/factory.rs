//! `IDBFactory` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::object::builtins::JsPromise;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::compare::compare_keys;

use crate::api::request::create_open_request;
use crate::convert::key::value_to_key;
use crate::driver::{OpenKind, PendingOpen};
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

/// Opens (or deletes) a database: creates the open-request shell, enqueues
/// the open, and schedules the pump. Everything else happens asynchronously.
fn issue_open(
    context: &mut Context,
    kind: OpenKind,
    name: String,
    version: u64,
) -> JsResult<JsObject> {
    let request_id = crate::driver::alloc_request_id(context);
    let conn_id = crate::driver::alloc_conn_id(context);
    let req_obj = create_open_request(request_id, context)?;
    crate::driver::enqueue_open(
        context,
        PendingOpen {
            kind,
            name,
            version,
            conn_id,
            request_id,
            upgrade_active: false,
            upgrade_txn_id: None,
            queued: false,
            blocked_fired: false,
            upgrade_versions: None,
        },
    );
    crate::runtime::schedule_pump(context);
    Ok(req_obj)
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
        // open(name, version?) -> IDBOpenDBRequest
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

                // Omitted/undefined version means "open current".
                let version = match args.get(1) {
                    Some(v) if !v.is_undefined() => Some(
                        crate::convert::webidl::to_unsigned_long_long_enforce_range(v, context)?,
                    ),
                    _ => None,
                };
                if version == Some(0) {
                    return Err(JsNativeError::typ()
                        .with_message("The version provided must not be 0.")
                        .into());
                }
                if context.get_data::<IdbRuntime>().is_none() {
                    return Err(JsNativeError::error()
                        .with_message("IndexedDB not initialized")
                        .into());
                }

                Ok(JsValue::from(issue_open(
                    context,
                    OpenKind::Open,
                    name,
                    version.unwrap_or(0),
                )?))
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

                if context.get_data::<IdbRuntime>().is_none() {
                    return Err(JsNativeError::error()
                        .with_message("IndexedDB not initialized")
                        .into());
                }

                Ok(JsValue::from(issue_open(
                    context,
                    OpenKind::Delete,
                    name,
                    0,
                )?))
            }),
        );

        // databases() -> Promise<sequence<DatabaseInfo>>
        class.method(
            js_string!("databases"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, context| {
                check_factory_brand(this)?;

                if context.get_data::<IdbRuntime>().is_none() {
                    return Err(JsNativeError::error()
                        .with_message("IndexedDB not initialized")
                        .into());
                }
                let list =
                    match crate::runtime::with_engine(context, |engine| engine.list_databases()) {
                        Ok(Ok(list)) => list,
                        Ok(Err(e)) => {
                            let exc = crate::dom::exception::create_dom_exception(
                                "UnknownError",
                                &e.to_string(),
                                context,
                            )?;
                            return Ok(JsValue::from(JsPromise::reject(
                                boa_engine::JsError::from_opaque(exc),
                                context,
                            )?));
                        }
                        Err(e) => return Err(e),
                    };

                // Snapshot; version-0 databases are omitted per spec.
                let arr: Vec<JsValue> = list
                    .iter()
                    .filter(|(_, version)| *version > 0)
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
                let array = boa_engine::object::builtins::JsArray::from_iter(arr, context);
                Ok(JsValue::from(JsPromise::resolve(array, context)?))
            }),
        );

        // cmp(first, second) -> -1 | 0 | 1
        class.method(
            js_string!("cmp"),
            2,
            NativeFunction::from_fn_ptr(|this, args, context| {
                check_factory_brand(this)?;

                // WebIDL requires two arguments; fewer is a TypeError.
                if args.len() < 2 {
                    return Err(JsNativeError::typ()
                        .with_message("IDBFactory.cmp requires two arguments")
                        .into());
                }

                let k1 = match value_to_key(&args[0], context) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, context));
                    }
                };
                let k2 = match value_to_key(&args[1], context) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, context));
                    }
                };

                Ok(JsValue::from(compare_keys(&k1, &k2) as i32))
            }),
        );

        Ok(())
    }
}
