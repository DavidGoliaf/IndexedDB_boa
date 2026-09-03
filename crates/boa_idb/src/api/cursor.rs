//! `IDBCursor` / `IDBCursorWithValue` implementation.

use boa_gc::{Finalize, Trace};

/// Cursor direction.
#[derive(Debug, Clone, PartialEq, Eq, Trace, Finalize)]
pub enum DirectionJs {
    /// Forward.
    Next,
    /// Forward, unique keys only.
    NextUnique,
    /// Backward.
    Prev,
    /// Backward, unique keys only.
    PrevUnique,
}

/// Native data for `IDBCursor`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBCursorData {
    /// Cursor identifier.
    pub cursor_id: u64,
    /// Cursor direction.
    #[unsafe_ignore_trace]
    pub direction: DirectionJs,
    /// Current key (encoded bytes).
    #[unsafe_ignore_trace]
    pub key: Vec<u8>,
    /// Current primary key (encoded bytes).
    #[unsafe_ignore_trace]
    pub primary_key: Vec<u8>,
    /// Whether the cursor has a value.
    pub has_value: bool,
}
