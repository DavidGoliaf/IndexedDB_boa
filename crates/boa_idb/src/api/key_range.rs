//! `IDBKeyRange` implementation.

use boa_gc::{Finalize, Trace};
use boa_idb_core::key::value::Key;

/// Native data for `IDBKeyRange`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBKeyRangeData {
    /// Lower bound (None = negative infinity).
    #[unsafe_ignore_trace]
    pub lower: Option<Key>,
    /// Upper bound (None = positive infinity).
    #[unsafe_ignore_trace]
    pub upper: Option<Key>,
    /// Whether the lower bound is open (exclusive).
    #[unsafe_ignore_trace]
    pub lower_open: bool,
    /// Whether the upper bound is open (exclusive).
    #[unsafe_ignore_trace]
    pub upper_open: bool,
}
