//! DOM shim classes for IndexedDB.
//!
//! Provides `EventTarget`, `Event`, `DOMException`, and `DOMStringList`
//! implementations required by the IndexedDB specification.

pub mod dispatch;
pub mod event;
pub mod event_target;
pub mod exception;
pub mod string_list;
