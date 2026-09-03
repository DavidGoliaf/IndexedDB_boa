//! `IDBCursor` / `IDBCursorWithValue` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::value::Key;
use boa_idb_core::proto::Direction;

use crate::convert::key::key_to_value;

/// Native data for `IDBCursor`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBCursorData {
    #[unsafe_ignore_trace]
    pub cursor_id: u64,
    #[unsafe_ignore_trace]
    pub direction: Direction,
    #[unsafe_ignore_trace]
    pub key: Option<Key>,
    #[unsafe_ignore_trace]
    pub primary_key: Option<Key>,
    pub request: Option<JsObject>,
    #[unsafe_ignore_trace]
    pub has_value: bool,
}

impl IdBCursorData {
    pub fn new(cursor_id: u64, direction: Direction) -> Self {
        Self {
            cursor_id,
            direction,
            key: None,
            primary_key: None,
            request: None,
            has_value: false,
        }
    }
}

/// `IDBCursor` class — uses IdBCursorData as native data.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBCursor;

impl Class for IdBCursor {
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
        let realm = class.context().realm().clone();

        let source_getter =
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()))
                .to_js_function(&realm);

        let direction_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBCursorData>() {
                    let dir_str = match data.direction {
                        Direction::Next => "next",
                        Direction::NextUnique => "nextunique",
                        Direction::Prev => "prev",
                        Direction::PrevUnique => "prevunique",
                    };
                    return Ok(JsValue::from(js_string!(dir_str)));
                }
            }
            Ok(JsValue::from(js_string!("next")))
        })
        .to_js_function(&realm);

        let key_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBCursorData>() {
                    if let Some(ref key) = data.key {
                        return key_to_value(key, ctx);
                    }
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let pk_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBCursorData>() {
                    if let Some(ref pk) = data.primary_key {
                        return key_to_value(pk, ctx);
                    }
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let request_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBCursorData>() {
                    if let Some(ref req) = data.request {
                        return Ok(JsValue::from(req.clone()));
                    }
                }
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

        class.method(
            js_string!("advance"),
            1,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("advance() not yet implemented")
                    .into())
            }),
        );

        class.method(
            js_string!("continue"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("continue() not yet implemented")
                    .into())
            }),
        );

        class.method(
            js_string!("continuePrimaryKey"),
            2,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("continuePrimaryKey() not yet implemented")
                    .into())
            }),
        );

        class.method(
            js_string!("update"),
            1,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("update() not yet implemented")
                    .into())
            }),
        );

        class.method(
            js_string!("delete"),
            0,
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                Err(JsNativeError::error()
                    .with_message("delete() not yet implemented")
                    .into())
            }),
        );

        Ok(())
    }
}

/// `IDBCursorWithValue` class.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBCursorWithValue;

impl Class for IdBCursorWithValue {
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
        let realm = class.context().realm().clone();

        let value_getter =
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| Ok(JsValue::undefined()))
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
