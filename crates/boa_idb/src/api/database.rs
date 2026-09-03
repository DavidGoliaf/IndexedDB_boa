//! `IDBDatabase` implementation.

use boa_gc::{Finalize, Trace};

/// Native data for `IDBDatabase`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBDatabaseData {
    /// Connection identifier.
    #[unsafe_ignore_trace]
    pub connection_id: u64,
    /// Database name.
    #[unsafe_ignore_trace]
    pub name: String,
    /// Database version.
    #[unsafe_ignore_trace]
    pub version: u64,
    /// Whether the connection is closed.
    pub closed: bool,
}
