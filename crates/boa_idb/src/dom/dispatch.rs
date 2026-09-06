//! DOM Event Dispatch algorithm (§5.9, §5.10, §2.1.1).

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};

use super::event::{AT_TARGET, BUBBLING_PHASE, CAPTURING_PHASE, EventDataHelper, NONE};
use super::event_target::{remove_listener_by_id, snapshot_listeners_full};

/// Dispatches an event to a target object.
/// Returns `true` if the event was not cancelled.
pub fn dispatch_event(
    this: &JsValue,
    event_val: &JsValue,
    context: &mut Context,
) -> JsResult<bool> {
    let event_obj = event_val.as_object().ok_or_else(|| {
        JsNativeError::typ().with_message("dispatchEvent requires an Event object")
    })?;

    let target = this
        .as_object()
        .ok_or_else(|| JsNativeError::typ().with_message("dispatchEvent called on non-object"))?;

    // Set the event's target
    if let Some(mut event_data) = event_obj.downcast_mut::<EventDataHelper>() {
        event_data.data.target = Some(target.clone());
        event_data.data.event_phase = AT_TARGET;
        event_data.data.is_trusted = false;
    }

    let path = build_event_path(&target, context);

    // 1. Capturing phase (root → parent of target)
    for ancestor in path.iter().rev() {
        if is_propagation_stopped(&event_obj) {
            return Ok(!is_default_prevented(&event_obj));
        }
        set_current_target(&event_obj, Some(ancestor.clone()), CAPTURING_PHASE);
        invoke_listeners(ancestor, &event_obj, Some(true), context)?;
        if is_propagation_stopped(&event_obj) {
            return Ok(!is_default_prevented(&event_obj));
        }
    }

    // 2. At-target phase — ALL listeners in registration order regardless of capture
    if is_propagation_stopped(&event_obj) {
        return Ok(!is_default_prevented(&event_obj));
    }
    set_current_target(&event_obj, Some(target.clone()), AT_TARGET);
    invoke_listeners(&target, &event_obj, None, context)?;
    if is_propagation_stopped(&event_obj) {
        return Ok(!is_default_prevented(&event_obj));
    }

    // 3. Bubbling phase (parent of target → root)
    let should_bubble = event_obj
        .downcast_ref::<EventDataHelper>()
        .is_some_and(|d| d.data.bubbles);

    if should_bubble {
        for ancestor in &path {
            if is_propagation_stopped(&event_obj) {
                return Ok(!is_default_prevented(&event_obj));
            }
            set_current_target(&event_obj, Some(ancestor.clone()), BUBBLING_PHASE);
            invoke_listeners(ancestor, &event_obj, Some(false), context)?;
            if is_propagation_stopped(&event_obj) {
                return Ok(!is_default_prevented(&event_obj));
            }
        }
    }

    // Reset
    set_current_target(&event_obj, None, NONE);

    Ok(!is_default_prevented(&event_obj))
}

fn build_event_path(target: &JsObject, context: &mut Context) -> Vec<JsObject> {
    let mut path = Vec::new();

    if let Ok(txn_val) = target.get(js_string!("transaction"), context) {
        if let Some(txn_obj) = txn_val.as_object() {
            path.push(txn_obj.clone());
            if let Ok(db_val) = txn_obj.get(js_string!("db"), context) {
                if let Some(db_obj) = db_val.as_object() {
                    path.push(db_obj.clone());
                }
            }
            return path;
        }
    }

    if let Ok(db_val) = target.get(js_string!("db"), context) {
        if let Some(db_obj) = db_val.as_object() {
            path.push(db_obj.clone());
        }
    }

    path
}

fn is_propagation_stopped(event: &JsObject) -> bool {
    event
        .downcast_ref::<EventDataHelper>()
        .is_some_and(|d| d.data.propagation_stopped)
}

fn is_default_prevented(event: &JsObject) -> bool {
    event
        .downcast_ref::<EventDataHelper>()
        .is_some_and(|d| d.data.default_prevented)
}

fn set_current_target(event: &JsObject, target: Option<JsObject>, phase: u8) {
    if let Some(mut data) = event.downcast_mut::<EventDataHelper>() {
        data.data.current_target = target;
        data.data.event_phase = phase;
    }
}

/// Invokes event listeners on a target for the given event.
///
/// `capture_filter`:
/// - `Some(true)` — only invoke capture listeners (capturing phase)
/// - `Some(false)` — only invoke non-capture listeners (bubbling phase)
/// - `None` — invoke ALL listeners in registration order (at-target phase)
fn invoke_listeners(
    target: &JsObject,
    event: &JsObject,
    capture_filter: Option<bool>,
    context: &mut Context,
) -> JsResult<()> {
    let event_type = event
        .downcast_ref::<EventDataHelper>()
        .map(|d| d.data.event_type.clone())
        .unwrap_or_default();

    let listeners = snapshot_listeners_full(target, &event_type)
        .into_iter()
        .filter(|(_, _, capture, _)| capture_filter.is_none_or(|cap| *capture == cap))
        .map(|(id, callback, _, once)| (id, callback, once))
        .collect::<Vec<_>>();

    for (id, callback, once) in listeners {
        // Check stopImmediatePropagation before each invocation
        if let Some(ed) = event.downcast_ref::<EventDataHelper>() {
            if ed.data.immediate_propagation_stopped {
                break;
            }
        }

        // Remove 'once' listeners before invoking (by registration id:
        // duplicate type/callback/capture registrations are rejected at
        // add time, so this removes exactly this registration).
        if once {
            remove_listener_by_id(target, id);
        }

        // Invoke the callback
        let result = if let Some(func) = callback.as_callable() {
            func.call(
                &JsValue::from(target.clone()),
                &[JsValue::from(event.clone())],
                context,
            )
        } else if let Some(obj) = callback.as_object() {
            // Handle event listener objects with handleEvent method
            if let Ok(handle_event) = obj.get(js_string!("handleEvent"), context) {
                if let Some(func) = handle_event.as_callable() {
                    func.call(
                        &JsValue::from(obj.clone()),
                        &[JsValue::from(event.clone())],
                        context,
                    )
                } else {
                    Ok(JsValue::undefined())
                }
            } else {
                Ok(JsValue::undefined())
            }
        } else {
            Ok(JsValue::undefined())
        };

        // If the listener threw, propagate the error
        result?;
    }

    Ok(())
}
