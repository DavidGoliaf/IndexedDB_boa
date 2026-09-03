//! Protocol types, identifiers, and operation definitions for IndexedDB engine.

use crate::clone::scvalue::ScValue;
use crate::key::range::EncodedRange;
use crate::key::value::Key;

/// Storage key identifier (analog of origin/tenant).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey(pub String);

impl StorageKey {
    /// Creates a new `StorageKey`.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

/// Store identifier.
pub type StoreId = u64;

/// Index identifier.
pub type IndexId = u64;

/// Transaction identifier.
pub type TxnId = u64;

/// Request identifier.
pub type RequestId = u64;

/// Cursor identifier.
pub type CursorId = u64;

/// Connection identifier.
pub type ConnectionId = u64;

/// IDB transaction mode (§2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxnMode {
    /// Read-only transaction.
    ReadOnly,
    /// Read-write transaction.
    ReadWrite,
    /// Version change transaction (schema modification).
    VersionChange,
}

/// Durability guarantee for transaction commit (§2.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Durability {
    /// Default durability (implementation-defined).
    #[default]
    Default,
    /// Strict durability (flush to disk on commit).
    Strict,
    /// Relaxed durability (may not flush immediately).
    Relaxed,
}

/// Cursor direction (§2.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Forward iteration.
    #[default]
    Next,
    /// Forward iteration, skipping duplicate keys.
    NextUnique,
    /// Backward iteration.
    Prev,
    /// Backward iteration, skipping duplicate keys.
    PrevUnique,
}

impl Direction {
    /// Returns `true` if the direction skips duplicate keys.
    pub fn is_unique(self) -> bool {
        matches!(self, Direction::NextUnique | Direction::PrevUnique)
    }

    /// Returns `true` if the direction is backward.
    pub fn is_prev(self) -> bool {
        matches!(self, Direction::Prev | Direction::PrevUnique)
    }
}

/// Source of an operation (store or index).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRef {
    /// Operation targets a store directly.
    Store(StoreId),
    /// Operation targets an index of a store.
    Index {
        /// The store containing the index.
        store: StoreId,
        /// The index to operate on.
        index: IndexId,
    },
}

/// Snapshot of a single IDB record (§2.12 IDBRecord).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordSnapshot {
    /// The key used for ordering (index key or primary key).
    pub key: Key,
    /// The primary key of the record.
    pub primary_key: Key,
    /// The stored value.
    pub value: ScValue,
}

/// Operations within an IDB transaction (§5, §6).
#[derive(Debug, Clone, PartialEq)]
pub enum Operation {
    /// Store or update a record.
    Put {
        /// Target store.
        store: StoreId,
        /// Explicit key (None if auto-generated or extracted from value).
        key: Option<Key>,
        /// Value to store.
        value: ScValue,
        /// If true, do not overwrite existing records (add() semantics).
        no_overwrite: bool,
    },
    /// Retrieve a single record by key.
    Get {
        /// Source (store or index).
        source: SourceRef,
        /// Key range to search.
        range: EncodedRange,
    },
    /// Retrieve a single key by key range.
    GetKey {
        /// Source (store or index).
        source: SourceRef,
        /// Key range to search.
        range: EncodedRange,
    },
    /// Retrieve all records matching a key range.
    GetAll {
        /// Source (store or index).
        source: SourceRef,
        /// Key range to search.
        range: EncodedRange,
        /// Maximum number of results.
        limit: Option<u32>,
        /// Cursor direction.
        direction: Direction,
    },
    /// Retrieve all keys matching a key range.
    GetAllKeys {
        /// Source (store or index).
        source: SourceRef,
        /// Key range to search.
        range: EncodedRange,
        /// Maximum number of results.
        limit: Option<u32>,
        /// Cursor direction.
        direction: Direction,
    },
    /// Retrieve all records with their keys matching a key range.
    GetAllRecords {
        /// Source (store or index).
        source: SourceRef,
        /// Key range to search.
        range: EncodedRange,
        /// Maximum number of results.
        limit: Option<u32>,
        /// Cursor direction.
        direction: Direction,
    },
    /// Delete records matching a key range.
    Delete {
        /// Target store.
        store: StoreId,
        /// Key range to delete.
        range: EncodedRange,
    },
    /// Clear all records from a store.
    Clear {
        /// Target store.
        store: StoreId,
    },
    /// Count records matching a key range.
    Count {
        /// Source (store or index).
        source: SourceRef,
        /// Key range to count.
        range: EncodedRange,
    },
    /// Open a cursor.
    OpenCursor {
        /// Source (store or index).
        source: SourceRef,
        /// Key range to iterate.
        range: EncodedRange,
        /// Cursor direction.
        direction: Direction,
        /// If true, only return keys without values.
        key_only: bool,
    },
    /// Advance a cursor by a count.
    CursorAdvance {
        /// Cursor to advance.
        cursor: CursorId,
        /// Number of steps to advance.
        count: u32,
    },
    /// Continue a cursor to the next key.
    CursorContinue {
        /// Cursor to continue.
        cursor: CursorId,
        /// Optional target key to continue to.
        target_key: Option<Key>,
    },
    /// Continue a cursor to a specific key and primary key.
    CursorContinuePrimaryKey {
        /// Cursor to continue.
        cursor: CursorId,
        /// Target key.
        target_key: Key,
        /// Target primary key.
        target_primary_key: Key,
    },
    /// Update the value at the cursor's current position.
    CursorUpdate {
        /// Cursor to update at.
        cursor: CursorId,
        /// New value.
        value: ScValue,
    },
    /// Delete the record at the cursor's current position.
    CursorDelete {
        /// Cursor to delete at.
        cursor: CursorId,
    },
}

/// Result of an operation execution.
#[derive(Debug, Clone, PartialEq)]
pub enum OpOutcome {
    /// No value returned (put, delete, clear).
    Empty,
    /// A single key returned.
    Key(Key),
    /// An optional value returned.
    Value(Option<ScValue>),
    /// Multiple keys returned.
    Keys(Vec<Key>),
    /// Multiple values returned.
    Values(Vec<ScValue>),
    /// Multiple records returned.
    Records(Vec<RecordSnapshot>),
    /// A count returned.
    Count(u64),
    /// A cursor was opened.
    CursorOpened {
        /// Cursor identifier.
        cursor_id: CursorId,
        /// Current key.
        key: Key,
        /// Current primary key.
        primary_key: Key,
        /// Current value (None for key_only cursors).
        value: Option<ScValue>,
    },
    /// A cursor was advanced.
    CursorAdvanced {
        /// Whether the cursor is at a valid position.
        has_value: bool,
        /// Current key (if valid).
        key: Option<Key>,
        /// Current primary key (if valid).
        primary_key: Option<Key>,
        /// Current value (if valid and not key_only).
        value: Option<ScValue>,
    },
}
