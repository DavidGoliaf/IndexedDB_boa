//! `EventTarget` implementation per DOM Living Standard.

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_gc::{Finalize, GcRefCell, Trace};

/// A registered event listener.
#[derive(Debug, Clone, Trace, Finalize)]
pub struct EventListenerEntry {
    #[unsafe_ignore_trace]
    pub event_type: String,
    pub callback: JsValue,
    #[unsafe_ignore_trace]
    pub capture: bool,
    #[unsafe_ignore_trace]
    pub once: bool,
    #[unsafe_ignore_trace]
    pub passive: bool,
    #[unsafe_ignore_trace]
    pub id: u64,
}

/// Native data for `EventTarget`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct EventTargetData {
    pub listeners: GcRefCell<Vec<EventListenerEntry>>,
    /// Counter for generating unique listener IDs.
    pub next_listener_id: std::cell::Cell<u64>,
}

impl EventTargetData {
    pub fn new() -> Self {
        Self {
            listeners: GcRefCell::default(),
            next_listener_id: std::cell::Cell::new(0),
        }
    }
}

/// `EventTarget` — not registered as a standalone class.
/// EventTarget methods are added to IDB classes via `add_event_target_methods`.
pub struct EventTarget;

/// Helper to add EventTarget methods to a class builder.
#[allow(clippy::too_many_lines)]
pub fn add_event_target_methods(class: &mut ClassBuilder<'_>) -> JsResult<()> {
    // addEventListener(type, callback, options)
    class.method(
        js_string!("addEventListener"),
        2,
        boa_engine::NativeFunction::from_fn_ptr(|this, args, context| {
            let event_type = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();

            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
            if callback.is_undefined() || callback.is_null() {
                return Ok(JsValue::undefined());
            }

            let mut capture = false;
            let mut once = false;
            let mut passive = false;

            if let Some(opts) = args.get(2) {
                if let Some(obj) = opts.as_object() {
                    if let Ok(c) = obj.get(js_string!("capture"), context) {
                        capture = c.to_boolean();
                    }
                    if let Ok(o) = obj.get(js_string!("once"), context) {
                        once = o.to_boolean();
                    }
                    if let Ok(p) = obj.get(js_string!("passive"), context) {
                        passive = p.to_boolean();
                    }
                } else {
                    capture = opts.to_boolean();
                }
            }

            if let Some(obj) = this.as_object() {
                // Duplicate (same type/callback/capture) registrations are ignored.
                // The id is computed before taking the mutable borrow: helpers
                // re-borrow the object, which would panic while borrowed.
                let id = next_listener_id(&obj);
                with_listeners_mut(&obj, |listeners| {
                    let duplicate = listeners.iter().any(|l| {
                        l.event_type == event_type
                            && values_equal(&l.callback, &callback)
                            && l.capture == capture
                    });
                    if !duplicate {
                        listeners.push(EventListenerEntry {
                            event_type,
                            callback,
                            capture,
                            once,
                            passive,
                            id,
                        });
                    }
                });
            }

            Ok(JsValue::undefined())
        }),
    );

    // removeEventListener(type, callback, options)
    class.method(
        js_string!("removeEventListener"),
        2,
        boa_engine::NativeFunction::from_fn_ptr(|this, args, context| {
            let event_type = args
                .first()
                .unwrap_or(&JsValue::undefined())
                .to_string(context)?
                .to_std_string_escaped();

            let callback = args.get(1).cloned().unwrap_or(JsValue::undefined());
            let mut capture = false;

            if let Some(opts) = args.get(2) {
                if let Some(obj) = opts.as_object() {
                    if let Ok(c) = obj.get(js_string!("capture"), context) {
                        capture = c.to_boolean();
                    }
                } else {
                    capture = opts.to_boolean();
                }
            }

            if let Some(obj) = this.as_object() {
                with_listeners_mut(&obj, |listeners| {
                    listeners.retain(|l| {
                        !(l.event_type == event_type
                            && values_equal(&l.callback, &callback)
                            && l.capture == capture)
                    });
                });
            }

            Ok(JsValue::undefined())
        }),
    );

    // dispatchEvent(event)
    class.method(
        js_string!("dispatchEvent"),
        1,
        boa_engine::NativeFunction::from_fn_ptr(|this, args, context| {
            let event = args
                .first()
                .ok_or_else(|| {
                    JsNativeError::typ().with_message("dispatchEvent requires an event argument")
                })?
                .as_object()
                .ok_or_else(|| JsNativeError::typ().with_message("Event must be an object"))?
                .clone();

            let not_cancelled =
                crate::dom::dispatch::dispatch_event(this, &JsValue::from(event), context)?;
            Ok(JsValue::from(not_cancelled))
        }),
    );

    Ok(())
}

#[allow(clippy::float_cmp)]
fn values_equal(a: &JsValue, b: &JsValue) -> bool {
    if a.is_null() && b.is_null() || a.is_undefined() && b.is_undefined() {
        return true;
    }
    if let (Some(a), Some(b)) = (a.as_boolean(), b.as_boolean()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (a.as_string(), b.as_string()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (a.as_i32(), b.as_i32()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (a.as_number(), b.as_number()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (a.as_object(), b.as_object()) {
        return a == b;
    }
    false
}

/// Equality helper used by event-listener removal across IDB classes.
pub fn listener_equal(a: &JsValue, b: &JsValue) -> bool {
    values_equal(a, b)
}

/// Runs `f` against the listener list of any IDB event target.
///
/// IDB objects carry their listeners in their own native data
/// (`IdBRequest`, `IdBTransaction`, `IdBDatabase`), not in a shared
/// `EventTargetData`, so dispatch and `addEventListener` must go through
/// this dispatcher instead of downcasting to a single type.
pub fn with_listeners_mut<R>(
    obj: &JsObject,
    f: impl FnOnce(&mut Vec<EventListenerEntry>) -> R,
) -> Option<R> {
    if let Some(data) = obj.downcast_ref::<crate::api::request::IdBOpenDBRequest>() {
        return Some(f(&mut data.request.listeners.borrow_mut()));
    }
    if let Some(data) = obj.downcast_ref::<crate::api::request::IdBRequest>() {
        return Some(f(&mut data.listeners.borrow_mut()));
    }
    if let Some(data) = obj.downcast_ref::<crate::api::transaction::IdBTransaction>() {
        return Some(f(&mut data.listeners.borrow_mut()));
    }
    if let Some(data) = obj.downcast_ref::<crate::api::database::IdBDatabase>() {
        return Some(f(&mut data.listeners.borrow_mut()));
    }
    if let Some(data) = obj.downcast_ref::<EventTargetData>() {
        let mut listeners = data.listeners.borrow_mut();
        return Some(f(&mut listeners));
    }
    None
}

/// Snapshots `(id, callback, once)` listeners of `event_type` in registration order.
pub fn snapshot_listeners(obj: &JsObject, event_type: &str) -> Vec<(u64, JsValue, bool)> {
    snapshot_listeners_full(obj, event_type)
        .into_iter()
        .map(|(id, callback, _, once)| (id, callback, once))
        .collect()
}

/// Snapshots `(id, callback, capture, once)` listeners of `event_type` in
/// registration order. Used by `dispatchEvent`, which must filter by phase
/// (capture/bubble/at-target) unlike the driver's at-target-only flow.
pub fn snapshot_listeners_full(
    obj: &JsObject,
    event_type: &str,
) -> Vec<(u64, JsValue, bool, bool)> {
    let mut out = Vec::new();
    with_listeners_mut(obj, |listeners| {
        // Next listener id for deduplication of re-added entries is kept
        // simple: capture order is registration order.
        for l in listeners.iter() {
            if l.event_type == event_type {
                out.push((l.id, l.callback.clone(), l.capture, l.once));
            }
        }
    });
    out
}

/// Removes a listener by its registration id.
pub fn remove_listener_by_id(obj: &JsObject, id: u64) {
    with_listeners_mut(obj, |listeners| {
        listeners.retain(|l| l.id != id);
    });
}

/// Next listener id for an object (max existing id + 1).
pub fn next_listener_id(obj: &JsObject) -> u64 {
    let mut next = 0;
    with_listeners_mut(obj, |listeners| {
        for l in listeners.iter() {
            next = next.max(l.id + 1);
        }
    });
    next
}
