//! `Event` implementation per DOM Living Standard.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};

pub const NONE: u8 = 0;
pub const CAPTURING_PHASE: u8 = 1;
pub const AT_TARGET: u8 = 2;
pub const BUBBLING_PHASE: u8 = 3;

/// Native data for `Event`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct EventData {
    #[unsafe_ignore_trace]
    pub event_type: String,
    #[unsafe_ignore_trace]
    pub bubbles: bool,
    #[unsafe_ignore_trace]
    pub cancelable: bool,
    pub default_prevented: bool,
    #[unsafe_ignore_trace]
    pub event_phase: u8,
    pub propagation_stopped: bool,
    pub immediate_propagation_stopped: bool,
    pub target: Option<JsObject>,
    pub current_target: Option<JsObject>,
    #[unsafe_ignore_trace]
    pub is_trusted: bool,
    #[unsafe_ignore_trace]
    pub time_stamp: f64,
}

impl EventData {
    pub fn new(event_type: String, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event_type,
            bubbles,
            cancelable,
            default_prevented: false,
            event_phase: NONE,
            propagation_stopped: false,
            immediate_propagation_stopped: false,
            target: None,
            current_target: None,
            is_trusted: false,
            time_stamp: 0.0,
        }
    }
}

/// Helper struct that wraps EventData for Class registration.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct EventDataHelper {
    pub data: EventData,
}

impl Class for EventDataHelper {
    const NAME: &'static str = "Event";
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

        let mut bubbles = false;
        let mut cancelable = false;

        if let Some(opts) = args.get(1) {
            if let Some(obj) = opts.as_object() {
                if let Ok(b) = obj.get(js_string!("bubbles"), context) {
                    bubbles = b.to_boolean();
                }
                if let Ok(c) = obj.get(js_string!("cancelable"), context) {
                    cancelable = c.to_boolean();
                }
            }
        }

        Ok(Self {
            data: EventData::new(event_type, bubbles, cancelable),
        })
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        let type_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    return Ok(JsValue::from(js_string!(data.data.event_type.as_str())));
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        let bubbles_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    return Ok(JsValue::from(data.data.bubbles));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        let cancelable_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    return Ok(JsValue::from(data.data.cancelable));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        let default_prevented_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    return Ok(JsValue::from(data.data.default_prevented));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        let event_phase_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    return Ok(JsValue::from(data.data.event_phase));
                }
            }
            Ok(JsValue::from(NONE))
        })
        .to_js_function(&realm);

        let target_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    if let Some(ref target) = data.data.target {
                        return Ok(JsValue::from(target.clone()));
                    }
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        let current_target_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    if let Some(ref ct) = data.data.current_target {
                        return Ok(JsValue::from(ct.clone()));
                    }
                }
            }
            Ok(JsValue::null())
        })
        .to_js_function(&realm);

        let is_trusted_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<EventDataHelper>() {
                    return Ok(JsValue::from(data.data.is_trusted));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("type"),
            Some(type_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("bubbles"),
            Some(bubbles_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("cancelable"),
            Some(cancelable_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("defaultPrevented"),
            Some(default_prevented_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("eventPhase"),
            Some(event_phase_getter),
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
            js_string!("isTrusted"),
            Some(is_trusted_getter),
            None,
            Attribute::READONLY,
        );

        class.method(
            js_string!("preventDefault"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, _ctx| {
                if let Some(obj) = this.as_object() {
                    if let Some(mut data) = obj.downcast_mut::<EventDataHelper>() {
                        if data.data.cancelable {
                            data.data.default_prevented = true;
                        }
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        class.method(
            js_string!("stopPropagation"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, _ctx| {
                if let Some(obj) = this.as_object() {
                    if let Some(mut data) = obj.downcast_mut::<EventDataHelper>() {
                        data.data.propagation_stopped = true;
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        class.method(
            js_string!("stopImmediatePropagation"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, _ctx| {
                if let Some(obj) = this.as_object() {
                    if let Some(mut data) = obj.downcast_mut::<EventDataHelper>() {
                        data.data.propagation_stopped = true;
                        data.data.immediate_propagation_stopped = true;
                    }
                }
                Ok(JsValue::undefined())
            }),
        );

        Ok(())
    }
}
