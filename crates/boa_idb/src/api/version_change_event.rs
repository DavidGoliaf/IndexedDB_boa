//! `IDBVersionChangeEvent` implementation.

use boa_gc::{Finalize, Trace};

/// Native data for `IDBVersionChangeEvent`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBVersionChangeEventData {
    /// Old database version.
    #[unsafe_ignore_trace]
    pub old_version: u64,
    /// New database version (None if database is being deleted).
    #[unsafe_ignore_trace]
    pub new_version: Option<u64>,
}
