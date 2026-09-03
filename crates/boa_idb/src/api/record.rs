//! `IDBRecord` implementation.

use boa_gc::{Finalize, Trace};

/// Native data for `IDBRecord`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBRecordData {
    /// The key (encoded bytes).
    #[unsafe_ignore_trace]
    pub key: Vec<u8>,
    /// The primary key (encoded bytes).
    #[unsafe_ignore_trace]
    pub primary_key: Vec<u8>,
    /// The stored value (encoded bytes).
    #[unsafe_ignore_trace]
    pub value: Vec<u8>,
}
