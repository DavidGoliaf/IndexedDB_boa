//! `IDBVersionChangeEvent` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};

/// Native data for `IDBVersionChangeEvent`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBVersionChangeEventData {
    #[unsafe_ignore_trace]
    pub old_version: u64,
    #[unsafe_ignore_trace]
    pub new_version: Option<u64>,
}

impl IdBVersionChangeEventData {
    pub fn new(old_version: u64, new_version: Option<u64>) -> Self {
        Self {
            old_version,
            new_version,
        }
    }
}

/// `IDBVersionChangeEvent` class.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBVersionChangeEvent;

impl Class for IdBVersionChangeEvent {
    const NAME: &'static str = "IDBVersionChangeEvent";
    const LENGTH: usize = 2;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<Self> {
        // Parse event type and options
        let _event_type = args
            .first()
            .unwrap_or(&JsValue::undefined())
            .to_string(context)?
            .to_std_string_escaped();

        Ok(IdBVersionChangeEvent)
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        let old_version_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBVersionChangeEventData>() {
                    return Ok(JsValue::from(data.old_version));
                }
            }
            Ok(JsValue::from(0))
        })
        .to_js_function(&realm);

        let new_version_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBVersionChangeEventData>() {
                    if let Some(nv) = data.new_version {
                        return Ok(JsValue::from(nv));
                    }
                    return Ok(JsValue::null());
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("oldVersion"),
            Some(old_version_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("newVersion"),
            Some(new_version_getter),
            None,
            Attribute::READONLY,
        );

        Ok(())
    }
}
