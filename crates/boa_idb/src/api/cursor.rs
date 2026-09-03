//! `IDBCursor` / `IDBCursorWithValue` implementation.

use boa_gc::{Finalize, Trace};
use boa_idb_core::key::value::Key;
use boa_idb_core::proto::Direction;

/// Native data for `IDBCursor`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBCursorData {
    /// Cursor identifier.
    #[unsafe_ignore_trace]
    pub cursor_id: u64,
    /// Cursor direction.
    #[unsafe_ignore_trace]
    pub direction: Direction,
    /// Current key.
    #[unsafe_ignore_trace]
    pub key: Key,
    /// Current primary key.
    #[unsafe_ignore_trace]
    pub primary_key: Key,
    /// Whether the cursor has a value.
    pub has_value: bool,
}
