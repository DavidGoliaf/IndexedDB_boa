//! `IDBVersionChangeEvent` implementation.

use crate::dom::event::EventData;
use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};

/// Native data for `IDBVersionChangeEvent`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBVersionChangeEventData {
    pub event_data: EventData,
    #[unsafe_ignore_trace]
    pub old_version: u64,
    #[unsafe_ignore_trace]
    pub new_version: Option<u64>,
}

impl IdBVersionChangeEventData {
    pub fn new(
        event_type: String,
        old_version: u64,
        new_version: Option<u64>,
        target: Option<JsObject>,
    ) -> Self {
        let mut data = EventData::new(event_type, false, false);
        data.target = target;
        Self {
            event_data: data,
            old_version,
            new_version,
        }
    }
}

/// `IDBVersionChangeEvent` class.
impl Class for IdBVersionChangeEventData {
    const NAME: &'static str = "IDBVersionChangeEvent";
    const LENGTH: usize = 2;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<Self> {
        let event_type = args
            .first()
            .unwrap_or(&JsValue::undefined())
            .to_string(context)?
            .to_std_string_escaped();

        let mut old_version = 0u64;
        let mut new_version = None;
        if let Some(init) = args.get(1) {
            if let Some(obj) = init.as_object() {
                if let Ok(v) = obj.get(js_string!("oldVersion"), context) {
                    if !v.is_undefined() {
                        let n = v.to_number(context)?;
                        if n.is_finite() && n >= 0.0 {
                            old_version = n.trunc() as u64;
                        }
                    }
                }
                if let Ok(v) = obj.get(js_string!("newVersion"), context) {
                    if !v.is_undefined() && !v.is_null() {
                        let n = v.to_number(context)?;
                        if n.is_finite() && n >= 0.0 {
                            new_version = Some(n.trunc() as u64);
                        }
                    }
                }
            }
        }
        Ok(Self::new(event_type, old_version, new_version, None))
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        let type_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBVersionChangeEventData>() {
                    return Ok(JsValue::from(js_string!(
                        data.event_data.event_type.as_str()
                    )));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let target_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBVersionChangeEventData>() {
                    if let Some(ref target) = data.event_data.target {
                        return Ok(JsValue::from(target.clone()));
                    }
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        let current_target_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBVersionChangeEventData>() {
                    if let Some(ref ct) = data.event_data.current_target {
                        return Ok(JsValue::from(ct.clone()));
                    }
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

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
            js_string!("type"),
            Some(type_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("target"),
            Some(target_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("currentTarget"),
            Some(current_target_getter),
            None,
            Attribute::READONLY,
        );
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
