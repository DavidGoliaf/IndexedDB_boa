//! `IDBVersionChangeEvent` implementation.

use boa_gc::{Finalize, Trace};

/// Native data for `IDBVersionChangeEvent`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBVersionChangeEventData {
    /// Old database version.
    pub old_version: u64,
    /// New database version (None if database is being deleted).
    pub new_version: Option<u64>,
}
