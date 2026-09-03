//! `IDBIndex` implementation.

use boa_engine::JsObject;
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::path::KeyPath;

/// Native data for `IDBIndex`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBIndexData {
    /// Index identifier.
    #[unsafe_ignore_trace]
    pub index_id: u64,
    /// Index name.
    #[unsafe_ignore_trace]
    pub name: String,
    /// Key path.
    #[unsafe_ignore_trace]
    pub key_path: KeyPath,
    /// Whether the index is unique.
    pub unique: bool,
    /// Whether the index is multi-entry.
    pub multi_entry: bool,
    /// Object store reference (GC-traced).
    pub object_store: JsObject,
}
