//! `IDBObjectStore` implementation.

use boa_engine::JsObject;
use boa_gc::{Finalize, Trace};
use boa_idb_core::key::path::KeyPath;

/// Native data for `IDBObjectStore`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBObjectStoreData {
    /// Store identifier.
    #[unsafe_ignore_trace]
    pub store_id: u64,
    /// Store name.
    #[unsafe_ignore_trace]
    pub name: String,
    /// Key path.
    #[unsafe_ignore_trace]
    pub key_path: KeyPath,
    /// Whether auto-increment is enabled.
    pub auto_increment: bool,
    /// Transaction reference (GC-traced).
    pub transaction: JsObject,
}
