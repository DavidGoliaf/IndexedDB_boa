//! `IDBRecord` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::value::Key;

use crate::convert::key::key_to_value;
use crate::convert::value::deserialize_from_storage;

/// Native data for `IDBRecord`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBRecordData {
    #[unsafe_ignore_trace]
    pub key: Key,
    #[unsafe_ignore_trace]
    pub primary_key: Key,
    #[unsafe_ignore_trace]
    pub value: boa_idb_core::clone::scvalue::ScValue,
}

impl IdBRecordData {
    pub fn new(key: Key, primary_key: Key, value: boa_idb_core::clone::scvalue::ScValue) -> Self {
        Self {
            key,
            primary_key,
            value,
        }
    }
}

/// Builds an `IDBRecord` object from a snapshot triple.
pub fn create_record_object(
    key: Key,
    primary_key: Key,
    value: boa_idb_core::clone::scvalue::ScValue,
    context: &mut Context,
) -> JsResult<JsObject> {
    IdBRecordData::from_data(IdBRecordData::new(key, primary_key, value), context)
}

impl Class for IdBRecordData {
    const NAME: &'static str = "IDBRecord";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBRecord cannot be constructed directly")
            .into())
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        let key_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRecordData>() {
                    return key_to_value(&data.key, ctx);
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let pk_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRecordData>() {
                    return key_to_value(&data.primary_key, ctx);
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let value_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBRecordData>() {
                    return deserialize_from_storage(&data.value, ctx);
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

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
            js_string!("value"),
            Some(value_getter),
            None,
            Attribute::READONLY,
        );

        Ok(())
    }
}
