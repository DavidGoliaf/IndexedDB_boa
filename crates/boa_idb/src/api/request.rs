//! `IDBRequest` / `IDBOpenDBRequest` implementation.

use boa_engine::JsObject;
use boa_gc::{Finalize, Trace};

/// Ready state for requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState {
    /// Request is pending.
    Pending,
    /// Request is done.
    Done,
}

/// Native data for `IDBRequest`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBRequestData {
    /// Request identifier.
    #[unsafe_ignore_trace]
    pub request_id: u64,
    /// Current ready state.
    #[unsafe_ignore_trace]
    pub ready_state: ReadyState,
    /// Source object (store, index, or cursor).
    pub source: Option<JsObject>,
    /// Transaction this request belongs to.
    pub transaction: Option<JsObject>,
}
