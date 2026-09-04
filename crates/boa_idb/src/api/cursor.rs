//! `IDBCursor` / `IDBCursorWithValue` implementation.
//!
//! Cursor objects are created by the driver when an open-cursor request
//! completes. Navigation reuses the opening request: the request is reset to
//! `pending` and the same request id is re-enqueued. Key, primary key and
//! value are read live from the driver cursor state.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::value::Key;
use boa_idb_core::proto::Direction;
use std::cmp::Ordering;

use crate::convert::key::{key_to_value, value_to_key};
use crate::convert::value::serialize_for_storage;
use crate::driver::{CursorAction, CursorView};
use crate::runtime::IdbRuntime;

/// Native data for `IDBCursor` (also carried by `IDBCursorWithValue`).
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBCursorData {
    /// Driver cursor id.
    #[unsafe_ignore_trace]
    pub cursor_id: u64,
    /// Request id that opened the cursor (reused for iteration).
    #[unsafe_ignore_trace]
    pub request_id: u64,
    /// Cursor direction (fixed at open).
    #[unsafe_ignore_trace]
    pub direction: Direction,
    /// Owning request object.
    pub request: Option<JsObject>,
}

impl IdBCursorData {
    pub fn new(cursor_id: u64, request_id: u64, direction: Direction) -> Self {
        Self {
            cursor_id,
            request_id,
            direction,
            request: None,
        }
    }
}

/// `IDBCursorWithValue` native data: shares the cursor data.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBCursorWithValueData(pub IdBCursorData);

/// Extracts `(cursor_id, direction, request)` from either cursor flavor.
fn cursor_identity(obj: &JsObject) -> Option<(u64, Direction, Option<JsObject>)> {
    if let Some(data) = obj.downcast_ref::<IdBCursorData>() {
        return Some((data.cursor_id, data.direction, data.request.clone()));
    }
    if let Some(wrapped) = obj.downcast_ref::<IdBCursorWithValueData>() {
        return Some((
            wrapped.0.cursor_id,
            wrapped.0.direction,
            wrapped.0.request.clone(),
        ));
    }
    None
}

/// Direction name.
fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Next => "next",
        Direction::NextUnique => "nextunique",
        Direction::Prev => "prev",
        Direction::PrevUnique => "prevunique",
    }
}

/// Snapshots the driver cursor view.
fn view_of(context: &Context, cursor_id: u64) -> Option<CursorView> {
    context.get_data::<IdbRuntime>().and_then(|runtime| {
        let d = crate::runtime::lock_mutex(&runtime.driver);
        crate::driver::cursor_view(&d, cursor_id)
    })
}

/// Requires a positioned cursor; throws `InvalidStateError` at the end.
fn require_position(view: &CursorView, context: &mut Context) -> JsResult<()> {
    if view.current.is_none() {
        return crate::dom::exception::throw_invalid_state_error(
            "The cursor is exhausted.",
            context,
        )
        .map(|_| ());
    }
    Ok(())
}

/// Requires a readwrite transaction for the cursor's transaction.
fn require_readwrite(context: &mut Context, txn_id: u64) -> JsResult<()> {
    let readonly = crate::runtime::txn_object(context, txn_id)
        .and_then(|obj| {
            obj.downcast_ref::<crate::api::transaction::IdBTransaction>()
                .map(|d| d.mode == boa_idb_core::proto::TxnMode::ReadOnly)
        })
        .unwrap_or(false);
    if readonly {
        return Err(crate::dom::exception::throw_idb_error(
            &boa_idb_core::error::IdbError::ReadOnly,
            context,
        ));
    }
    Ok(())
}

/// Reuses the opening request for one iteration step.
fn reuse_request(context: &mut Context, view: &CursorView, action: CursorAction) -> JsResult<()> {
    let request = crate::runtime::request_object(context, view.request_id)
        .ok_or_else(|| JsNativeError::error().with_message("Cursor request is gone"))?;
    crate::api::request::with_request_mut(&request, |req| {
        req.ready_state = crate::api::request::ReadyState::Pending;
        req.result = None;
        req.error = None;
    });
    crate::driver::enqueue_op(
        context,
        view.txn_id,
        view.request_id,
        crate::driver::PendingOp::CursorOp {
            cursor_id: view.cursor_id,
            action,
        },
    );
    crate::runtime::schedule_pump(context);
    Ok(())
}

/// Shared getters installed on both cursor classes.
fn install_cursor_accessors(class: &mut ClassBuilder<'_>) -> JsResult<()> {
    let realm = class.context().realm().clone();

    let source_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
        if let Some(obj) = this.as_object()
            && let Some((_, _, request)) = cursor_identity(&obj)
            && let Some(req_obj) = request
            && let Some(source) =
                crate::api::request::with_request_ref(&req_obj, |d| d.source.clone()).flatten()
        {
            return Ok(JsValue::from(source));
        }
        Ok(JsValue::undefined())
    })
    .to_js_function(&realm);

    let direction_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
        if let Some(obj) = this.as_object()
            && let Some((_, direction, _)) = cursor_identity(&obj)
        {
            return Ok(JsValue::from(js_string!(direction_name(direction))));
        }
        Ok(JsValue::from(js_string!("next")))
    })
    .to_js_function(&realm);

    let key_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
        if let Some(obj) = this.as_object()
            && let Some((cursor_id, _, _)) = cursor_identity(&obj)
            && let Some(view) = view_of(ctx, cursor_id)
            && let Some(row) = view.current
        {
            return key_to_value(&row.key, ctx);
        }
        Ok(JsValue::undefined())
    })
    .to_js_function(&realm);

    let pk_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
        if let Some(obj) = this.as_object()
            && let Some((cursor_id, _, _)) = cursor_identity(&obj)
            && let Some(view) = view_of(ctx, cursor_id)
            && let Some(row) = view.current
        {
            return key_to_value(&row.primary_key, ctx);
        }
        Ok(JsValue::undefined())
    })
    .to_js_function(&realm);

    let request_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
        if let Some(obj) = this.as_object()
            && let Some((_, _, request)) = cursor_identity(&obj)
            && let Some(req) = request
        {
            return Ok(JsValue::from(req));
        }
        Ok(JsValue::undefined())
    })
    .to_js_function(&realm);

    class.accessor(
        js_string!("source"),
        Some(source_getter),
        None,
        Attribute::READONLY,
    );
    class.accessor(
        js_string!("direction"),
        Some(direction_getter),
        None,
        Attribute::READONLY,
    );
    class.accessor(
        js_string!("key"),
        Some(key_getter),
        None,
        Attribute::READONLY,
    );
    class.accessor(
        js_string!("primaryKey"),
        Some(pk_getter),
        None,
        Attribute::READONLY,
    );
    class.accessor(
        js_string!("request"),
        Some(request_getter),
        None,
        Attribute::READONLY,
    );
    Ok(())
}

/// Shared navigation/update/delete methods.
#[allow(clippy::too_many_lines)]
fn install_cursor_methods(class: &mut ClassBuilder<'_>) -> JsResult<()> {
    // advance(count)
    class.method(
        js_string!("advance"),
        1,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let obj = this
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let (cursor_id, _, _) = cursor_identity(&obj)
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let count_val = args.first().ok_or_else(|| {
                JsNativeError::typ().with_message("advance requires a count argument")
            })?;
            let count = crate::convert::webidl::to_unsigned_long_enforce_range(count_val, context)?;
            if count == 0 {
                return Err(JsNativeError::typ()
                    .with_message("advance count must not be 0")
                    .into());
            }
            let view = view_of(context, cursor_id)
                .ok_or_else(|| JsNativeError::error().with_message("Cursor is gone"))?;
            crate::api::support::require_live_cursor(context, view.cursor_id, view.txn_id)?;
            require_position(&view, context)?;
            reuse_request(context, &view, CursorAction::Advance(count))?;
            Ok(JsValue::undefined())
        }),
    );

    // continue(key?)
    class.method(
        js_string!("continue"),
        0,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let obj = this
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let (cursor_id, _, _) = cursor_identity(&obj)
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let view = view_of(context, cursor_id)
                .ok_or_else(|| JsNativeError::error().with_message("Cursor is gone"))?;
            crate::api::support::require_live_cursor(context, view.cursor_id, view.txn_id)?;
            require_position(&view, context)?;
            let Some(current) = view.current.clone() else {
                // `require_position` above guarantees this; the branch
                // satisfies the no-panic policy without `expect`.
                return crate::dom::exception::throw_invalid_state_error(
                    "The cursor is exhausted.",
                    context,
                );
            };
            let key_arg = args.first().cloned().unwrap_or(JsValue::undefined());
            let action = if key_arg.is_undefined() {
                CursorAction::Continue(None)
            } else {
                let target = value_to_key(&key_arg, context)
                    .map_err(|e| crate::convert::key::throw_key_conversion_error(e, context))?;
                check_continue_key(&view, &current.key, &target, context)?;
                CursorAction::Continue(Some(target))
            };
            reuse_request(context, &view, action)?;
            Ok(JsValue::undefined())
        }),
    );

    // continuePrimaryKey(key, primaryKey)
    class.method(
        js_string!("continuePrimaryKey"),
        2,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let obj = this
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let (cursor_id, _, _) = cursor_identity(&obj)
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let view = view_of(context, cursor_id)
                .ok_or_else(|| JsNativeError::error().with_message("Cursor is gone"))?;
            crate::api::support::require_live_cursor(context, view.cursor_id, view.txn_id)?;
            require_position(&view, context)?;
            if !matches!(view.direction, Direction::Next | Direction::Prev) {
                return Err(crate::dom::exception::throw_idb_error(
                    &boa_idb_core::error::IdbError::InvalidAccess(
                        "continuePrimaryKey requires next/prev direction".into(),
                    ),
                    context,
                ));
            }
            if !view.is_index {
                return Err(crate::dom::exception::throw_idb_error(
                    &boa_idb_core::error::IdbError::InvalidAccess(
                        "continuePrimaryKey requires an index cursor".into(),
                    ),
                    context,
                ));
            }
            if args.len() < 2 {
                return Err(JsNativeError::typ()
                    .with_message("continuePrimaryKey requires two arguments")
                    .into());
            }
            let key = value_to_key(&args[0], context)
                .map_err(|e| crate::convert::key::throw_key_conversion_error(e, context))?;
            let primary_key = value_to_key(&args[1], context)
                .map_err(|e| crate::convert::key::throw_key_conversion_error(e, context))?;
            check_continue_primary_key(&view, &key, &primary_key, context)?;
            reuse_request(
                context,
                &view,
                CursorAction::ContinuePrimaryKey { key, primary_key },
            )?;
            Ok(JsValue::undefined())
        }),
    );

    // update(value)
    class.method(
        js_string!("update"),
        1,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let obj = this
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let (cursor_id, _, _) = cursor_identity(&obj)
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let view = view_of(context, cursor_id)
                .ok_or_else(|| JsNativeError::error().with_message("Cursor is gone"))?;
            crate::api::support::require_live_cursor(context, view.cursor_id, view.txn_id)?;
            require_position(&view, context)?;
            if view.key_only {
                return crate::dom::exception::throw_invalid_state_error(
                    "update() is not available on key cursors.",
                    context,
                );
            }
            require_readwrite(context, view.txn_id)?;
            let default_val = JsValue::undefined();
            let value_js = args.first().unwrap_or(&default_val);
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
            if !matches!(view.key_path, boa_idb_core::key::path::KeyPath::Empty)
                && let Err(error) = view.key_path.extract(&sc_value)
            {
                return Err(crate::convert::key::throw_key_conversion_error(
                    error.into(),
                    context,
                ));
            }
            reuse_request(context, &view, CursorAction::Update(sc_value))?;
            Ok(JsValue::undefined())
        }),
    );

    // delete()
    class.method(
        js_string!("delete"),
        0,
        NativeFunction::from_fn_ptr(|this, _args, context| {
            let obj = this
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let (cursor_id, _, _) = cursor_identity(&obj)
                .ok_or_else(|| JsNativeError::typ().with_message("'this' is not an IDBCursor"))?;
            let view = view_of(context, cursor_id)
                .ok_or_else(|| JsNativeError::error().with_message("Cursor is gone"))?;
            crate::api::support::require_live_cursor(context, view.cursor_id, view.txn_id)?;
            require_position(&view, context)?;
            if view.key_only {
                return crate::dom::exception::throw_invalid_state_error(
                    "delete() is not available on key cursors.",
                    context,
                );
            }
            require_readwrite(context, view.txn_id)?;
            reuse_request(context, &view, CursorAction::Delete)?;
            Ok(JsValue::undefined())
        }),
    );

    Ok(())
}

/// Validates the strict direction ordering required by continuePrimaryKey().
fn check_continue_primary_key(
    view: &crate::driver::CursorView,
    key: &Key,
    primary_key: &Key,
    context: &mut Context,
) -> JsResult<()> {
    let Some(current) = view.current.as_ref() else {
        return Ok(());
    };
    let key_order = boa_idb_core::key::compare::compare_keys(key, &current.key);
    let valid = match view.direction {
        Direction::Next => {
            key_order == Ordering::Greater
                || (key_order == Ordering::Equal
                    && boa_idb_core::key::compare::compare_keys(primary_key, &current.primary_key)
                        == Ordering::Greater)
        }
        Direction::Prev => {
            key_order == Ordering::Less
                || (key_order == Ordering::Equal
                    && boa_idb_core::key::compare::compare_keys(primary_key, &current.primary_key)
                        == Ordering::Less)
        }
        Direction::NextUnique | Direction::PrevUnique => false,
    };
    if valid {
        Ok(())
    } else {
        crate::dom::exception::throw_data_error(
            "continuePrimaryKey target must advance in cursor direction.",
            context,
        )
        .map(|_| ())
    }
}

/// Validates a `continue(key)` target against the direction and position.
fn check_continue_key(
    view: &CursorView,
    current: &Key,
    target: &Key,
    context: &mut Context,
) -> JsResult<()> {
    use boa_idb_core::key::compare::compare_keys;
    let ord = compare_keys(target, current);
    let unique = matches!(
        view.direction,
        Direction::NextUnique | Direction::PrevUnique
    );
    let ok = match view.direction {
        Direction::Next | Direction::NextUnique => {
            ord == Ordering::Greater || (unique && ord == Ordering::Equal)
        }
        Direction::Prev | Direction::PrevUnique => {
            ord == Ordering::Less || (unique && ord == Ordering::Equal)
        }
    };
    if !ok {
        return Err(crate::dom::exception::throw_idb_error(
            &boa_idb_core::error::IdbError::Data(
                "continue() key is not in the cursor direction".into(),
            ),
            context,
        ));
    }
    Ok(())
}

impl Class for IdBCursorData {
    const NAME: &'static str = "IDBCursor";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBCursor cannot be constructed directly")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        install_cursor_accessors(class)?;
        install_cursor_methods(class)?;
        Ok(())
    }
}

impl Class for IdBCursorWithValueData {
    const NAME: &'static str = "IDBCursorWithValue";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBCursorWithValue cannot be constructed directly")
            .into())
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        install_cursor_accessors(class)?;
        install_cursor_methods(class)?;

        // `value` exists only on IDBCursorWithValue.
        let realm = class.context().realm().clone();
        let value_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object()
                && let Some((cursor_id, _, _)) = cursor_identity(&obj)
                && let Some(view) = view_of(ctx, cursor_id)
                && let Some(row) = view.current
                && let Some(value) = row.value
            {
                return crate::convert::value::deserialize_from_storage(&value, ctx);
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);
        class.accessor(
            js_string!("value"),
            Some(value_getter),
            None,
            Attribute::READONLY,
        );
        Ok(())
    }
}
