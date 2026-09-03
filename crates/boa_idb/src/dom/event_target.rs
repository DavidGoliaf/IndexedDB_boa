//! `EventTarget` implementation.

use boa_gc::{Finalize, GcRefCell, Trace};

/// A listener registration.
#[derive(Debug, Clone, Trace, Finalize)]
pub struct EventListener {
    pub capture: bool,
    pub once: bool,
}

/// Native data for `EventTarget`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct EventTargetData {
    #[unsafe_ignore_trace]
    pub listeners: GcRefCell<Vec<(String, EventListener)>>,
}

impl EventTargetData {
    pub fn new() -> Self {
        Self {
            listeners: GcRefCell::default(),
        }
    }
}
