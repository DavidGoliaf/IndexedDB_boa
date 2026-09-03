//! `DOMStringList` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};

/// Native data for `DOMStringList`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct DomStringListData {
    #[unsafe_ignore_trace]
    pub items: Vec<String>,
}

impl DomStringListData {
    pub fn new(items: Vec<String>) -> Self {
        Self { items }
    }

    pub fn length(&self) -> u32 {
        self.items.len() as u32
    }

    pub fn item(&self, index: u32) -> Option<&str> {
        self.items.get(index as usize).map(|s| s.as_str())
    }

    pub fn contains(&self, value: &str) -> bool {
        self.items.iter().any(|s| s == value)
    }
}

/// Helper to add DOMStringList methods to a class builder that uses DomStringListData.
pub fn add_dom_string_list_methods(class: &mut ClassBuilder<'_>) -> JsResult<()> {
    let realm = class.context().realm().clone();

    let length_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
        if let Some(obj) = this.as_object() {
            if let Some(data) = obj.downcast_ref::<DomStringListData>() {
                return Ok(JsValue::from(data.length()));
            }
        }
        Ok(JsValue::from(0))
    })
    .to_js_function(&realm);

    class.accessor(
        js_string!("length"),
        Some(length_getter),
        None,
        Attribute::READONLY,
    );

    class.method(
        js_string!("item"),
        1,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let index = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_u32(context)?;

            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<DomStringListData>() {
                    if let Some(s) = data.item(index) {
                        return Ok(JsValue::from(js_string!(s)));
                    }
                    return Ok(JsValue::undefined());
                }
            }
            Ok(JsValue::undefined())
        }),
    );

    class.method(
        js_string!("contains"),
        1,
        NativeFunction::from_fn_ptr(|this, args, context| {
            let value = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();

            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<DomStringListData>() {
                    return Ok(JsValue::from(data.contains(&value)));
                }
            }
            Ok(JsValue::from(false))
        }),
    );

    Ok(())
}

/// `DOMStringList` class (constructible only by the implementation).
impl Class for DomStringListData {
    const NAME: &'static str = "DOMStringList";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("DOMStringList cannot be constructed directly")
            .into())
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        add_dom_string_list_methods(class)
    }
}

/// Builds a `DOMStringList` object from implementation-side names.
pub fn dom_string_list(items: Vec<String>, context: &mut Context) -> JsResult<JsObject> {
    DomStringListData::from_data(DomStringListData::new(items), context)
}
