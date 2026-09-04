//! `IDBKeyRange` implementation.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::native_function::NativeFunction;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsResult, JsValue, js_string};
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::compare::compare_keys;
use boa_idb_core::key::value::Key;
use std::cmp::Ordering;

use crate::convert::key::{key_to_value, value_to_key};

/// Native data for `IDBKeyRange`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBKeyRange {
    #[unsafe_ignore_trace]
    pub lower: Option<Key>,
    #[unsafe_ignore_trace]
    pub upper: Option<Key>,
    #[unsafe_ignore_trace]
    pub lower_open: bool,
    #[unsafe_ignore_trace]
    pub upper_open: bool,
}

impl IdBKeyRange {
    pub fn new(lower: Option<Key>, upper: Option<Key>, lower_open: bool, upper_open: bool) -> Self {
        Self {
            lower,
            upper,
            lower_open,
            upper_open,
        }
    }
}

fn create_key_range(data: IdBKeyRange, context: &mut Context) -> JsResult<boa_engine::JsObject> {
    let obj = IdBKeyRange::from_data(data, context)?;
    obj.set(
        js_string!("__boa_idb_key_range"),
        JsValue::from(true),
        false,
        context,
    )?;
    Ok(obj)
}

impl Class for IdBKeyRange {
    const NAME: &'static str = "IDBKeyRange";
    const LENGTH: usize = 0;
    const ATTRIBUTES: Attribute = Attribute::all();

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("IDBKeyRange cannot be constructed directly. Use only(), lowerBound(), upperBound(), or bound()")
            .into())
    }

    #[allow(clippy::too_many_lines)]
    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let realm = class.context().realm().clone();

        // lower getter
        let lower_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBKeyRange>() {
                    if let Some(ref key) = data.lower {
                        return key_to_value(key, ctx);
                    }
                    return Ok(JsValue::undefined());
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        // upper getter
        let upper_getter = NativeFunction::from_fn_ptr(|this, _args, ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBKeyRange>() {
                    if let Some(ref key) = data.upper {
                        return key_to_value(key, ctx);
                    }
                    return Ok(JsValue::undefined());
                }
            }
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);

        // lowerOpen getter
        let lower_open_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBKeyRange>() {
                    return Ok(JsValue::from(data.lower_open));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        // upperOpen getter
        let upper_open_getter = NativeFunction::from_fn_ptr(|this, _args, _ctx| {
            if let Some(obj) = this.as_object() {
                if let Some(data) = obj.downcast_ref::<IdBKeyRange>() {
                    return Ok(JsValue::from(data.upper_open));
                }
            }
            Ok(JsValue::from(false))
        })
        .to_js_function(&realm);

        class.accessor(
            js_string!("lower"),
            Some(lower_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("upper"),
            Some(upper_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("lowerOpen"),
            Some(lower_open_getter),
            None,
            Attribute::READONLY,
        );
        class.accessor(
            js_string!("upperOpen"),
            Some(upper_open_getter),
            None,
            Attribute::READONLY,
        );

        // includes(key) method
        class.method(
            js_string!("includes"),
            1,
            NativeFunction::from_fn_ptr(|this, args, ctx| {
                if args.is_empty() {
                    return Err(crate::dom::exception::throw_type_error(
                        "IDBKeyRange.includes requires a key argument",
                    ));
                }
                let key = match value_to_key(&args[0], ctx) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, ctx));
                    }
                };

                if let Some(obj) = this.as_object() {
                    if let Some(data) = obj.downcast_ref::<IdBKeyRange>() {
                        let in_lower = match &data.lower {
                            Some(lower) => {
                                let cmp = compare_keys(lower, &key);
                                if data.lower_open {
                                    cmp == Ordering::Less
                                } else {
                                    cmp != Ordering::Greater
                                }
                            }
                            None => true,
                        };
                        let in_upper = match &data.upper {
                            Some(upper) => {
                                let cmp = compare_keys(&key, upper);
                                if data.upper_open {
                                    cmp == Ordering::Less
                                } else {
                                    cmp != Ordering::Greater
                                }
                            }
                            None => true,
                        };
                        return Ok(JsValue::from(in_lower && in_upper));
                    }
                }
                Ok(JsValue::from(false))
            }),
        );

        // Static methods: only, lowerBound, upperBound, bound

        // IDBKeyRange.only(key)
        class.static_method(
            js_string!("only"),
            1,
            NativeFunction::from_fn_ptr(|_this, args, ctx| {
                if args.is_empty() {
                    return Err(crate::dom::exception::throw_type_error(
                        "IDBKeyRange.only requires a key argument",
                    ));
                }
                let key = match value_to_key(&args[0], ctx) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, ctx));
                    }
                };
                let data = IdBKeyRange {
                    lower: Some(key.clone()),
                    upper: Some(key),
                    lower_open: false,
                    upper_open: false,
                };
                let obj = create_key_range(data, ctx)?;
                Ok(JsValue::from(obj))
            }),
        );

        // IDBKeyRange.lowerBound(key, open)
        class.static_method(
            js_string!("lowerBound"),
            1,
            NativeFunction::from_fn_ptr(|_this, args, ctx| {
                if args.is_empty() {
                    return Err(crate::dom::exception::throw_type_error(
                        "IDBKeyRange.lowerBound requires a key argument",
                    ));
                }
                let key = match value_to_key(&args[0], ctx) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, ctx));
                    }
                };
                let open = args.get(1).is_some_and(|v| v.to_boolean());
                let data = IdBKeyRange {
                    lower: Some(key),
                    upper: None,
                    lower_open: open,
                    upper_open: true,
                };
                let obj = create_key_range(data, ctx)?;
                Ok(JsValue::from(obj))
            }),
        );

        // IDBKeyRange.upperBound(key, open)
        class.static_method(
            js_string!("upperBound"),
            1,
            NativeFunction::from_fn_ptr(|_this, args, ctx| {
                if args.is_empty() {
                    return Err(crate::dom::exception::throw_type_error(
                        "IDBKeyRange.upperBound requires a key argument",
                    ));
                }
                let key = match value_to_key(&args[0], ctx) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, ctx));
                    }
                };
                let open = args.get(1).is_some_and(|v| v.to_boolean());
                let data = IdBKeyRange {
                    lower: None,
                    upper: Some(key),
                    lower_open: true,
                    upper_open: open,
                };
                let obj = create_key_range(data, ctx)?;
                Ok(JsValue::from(obj))
            }),
        );

        // IDBKeyRange.bound(lower, upper, lowerOpen, upperOpen)
        class.static_method(
            js_string!("bound"),
            2,
            NativeFunction::from_fn_ptr(|_this, args, ctx| {
                if args.len() < 2 {
                    return Err(crate::dom::exception::throw_type_error(
                        "IDBKeyRange.bound requires two key arguments",
                    ));
                }
                let lower = match value_to_key(&args[0], ctx) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, ctx));
                    }
                };
                let upper = match value_to_key(&args[1], ctx) {
                    Ok(key) => key,
                    Err(e) => {
                        return Err(crate::convert::key::throw_key_conversion_error(e, ctx));
                    }
                };
                if compare_keys(&lower, &upper) == Ordering::Greater {
                    return crate::dom::exception::throw_data_error(
                        "The lower key is greater than the upper key",
                        ctx,
                    );
                }
                let lower_open = args.get(2).is_some_and(|v| v.to_boolean());
                let upper_open = args.get(3).is_some_and(|v| v.to_boolean());
                let data = IdBKeyRange {
                    lower: Some(lower),
                    upper: Some(upper),
                    lower_open,
                    upper_open,
                };
                let obj = create_key_range(data, ctx)?;
                Ok(JsValue::from(obj))
            }),
        );

        Ok(())
    }
}
