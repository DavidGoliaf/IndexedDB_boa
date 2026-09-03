//! `Event` implementation.

use boa_gc::{Finalize, Trace};

/// Native data for `Event`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct EventData {
    #[unsafe_ignore_trace]
    pub event_type: String,
    pub bubbles: bool,
    pub cancelable: bool,
    pub default_prevented: bool,
    pub event_phase: u8,
    pub propagation_stopped: bool,
}

impl EventData {
    pub fn new(event_type: String, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event_type,
            bubbles,
            cancelable,
            default_prevented: false,
            event_phase: 0,
            propagation_stopped: false,
        }
    }
}
