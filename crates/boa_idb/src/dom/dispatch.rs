//! DOM Event Dispatch algorithm.

use boa_engine::{Context, JsObject, JsResult};

/// Dispatches an event to a target object.
pub fn dispatch_event(
    _target: &JsObject,
    _event: &JsObject,
    _context: &mut Context,
) -> JsResult<bool> {
    // TODO: implement full event dispatch with capturing/bubbling
    Ok(true)
}
