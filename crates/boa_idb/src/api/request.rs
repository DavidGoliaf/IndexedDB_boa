//! `IDBRequest` / `IDBOpenDBRequest` implementation.

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue};
use boa_gc::{Finalize, Trace};

/// Ready state for requests.
#[derive(Debug, Clone, PartialEq, Eq, Trace, Finalize)]
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
    pub request_id: u64,
    /// Current ready state.
    pub ready_state: ReadyState,
    /// Result value (set when done).
    pub result: Option<JsValue>,
    /// Error (set when done with error).
    pub error: Option<JsObject>,
    /// Source object (store, index, or cursor).
    pub source: Option<JsObject>,
    /// Transaction this request belongs to.
    pub transaction: Option<JsObject>,
}

/// Native data for `IDBOpenDBRequest`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBOpenDBRequestData {
    /// Base request data.
    pub base: IdBRequestData,
}
