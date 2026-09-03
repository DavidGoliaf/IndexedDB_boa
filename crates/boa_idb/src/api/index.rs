//! `IDBIndex` implementation.

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue};
use boa_gc::{Finalize, Trace};

/// Native data for `IDBIndex`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBIndexData {
    /// Index identifier.
    pub index_id: u64,
    /// Index name.
    pub name: String,
    /// Whether the index is unique.
    pub unique: bool,
    /// Whether the index is multi-entry.
    pub multi_entry: bool,
}
