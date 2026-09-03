//! `IDBRecord` implementation.

use boa_gc::{Finalize, Trace};
use boa_idb_core::clone::scvalue::ScValue;
use boa_idb_core::key::value::Key;

/// Native data for `IDBRecord`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBRecordData {
    /// The key used for ordering (index key or primary key).
    #[unsafe_ignore_trace]
    pub key: Key,
    /// The primary key of the record.
    #[unsafe_ignore_trace]
    pub primary_key: Key,
    /// The stored value.
    #[unsafe_ignore_trace]
    pub value: ScValue,
}
