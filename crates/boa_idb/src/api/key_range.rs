//! `IDBKeyRange` implementation.

use boa_gc::{Finalize, Trace};

/// Native data for `IDBKeyRange`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBKeyRangeData {
    #[unsafe_ignore_trace]
    pub lower: Option<Vec<u8>>,
    #[unsafe_ignore_trace]
    pub upper: Option<Vec<u8>>,
    pub lower_open: bool,
    pub upper_open: bool,
}
