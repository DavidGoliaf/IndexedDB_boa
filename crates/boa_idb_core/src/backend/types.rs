//! Type definitions for backend metadata and specifications.

use crate::key::path::KeyPath;
use crate::key::utf16::Utf16String;
use crate::proto::{IndexId, StoreId};

/// Specification for creating a new object store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSpec {
    /// Store name.
    pub name: Utf16String,
    /// Key path for extracting keys from values.
    pub key_path: KeyPath,
    /// Whether to auto-generate keys.
    pub auto_increment: bool,
}

/// Specification for creating a new index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSpec {
    /// Index name.
    pub name: Utf16String,
    /// Key path for extracting index keys from values.
    pub key_path: KeyPath,
    /// Whether the index enforces uniqueness.
    pub unique: bool,
    /// Whether the index supports multi-entry (array) keys.
    pub multi_entry: bool,
}

/// Metadata for an existing index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMeta {
    /// Index identifier.
    pub id: IndexId,
    /// Store containing this index.
    pub store_id: StoreId,
    /// Index name.
    pub name: Utf16String,
    /// Key path for extracting index keys.
    pub key_path: KeyPath,
    /// Whether the index enforces uniqueness.
    pub unique: bool,
    /// Whether the index supports multi-entry keys.
    pub multi_entry: bool,
    /// Whether the index has been deleted (soft delete during version change).
    pub deleted: bool,
}

/// Metadata for an existing object store.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreMeta {
    /// Store identifier.
    pub id: StoreId,
    /// Store name.
    pub name: Utf16String,
    /// Key path for extracting keys.
    pub key_path: KeyPath,
    /// Whether auto-increment is enabled.
    pub auto_increment: bool,
    /// Current key generator value.
    pub key_gen: f64,
    /// Indexes belonging to this store.
    pub indexes: Vec<IndexMeta>,
    /// Whether the store has been deleted (soft delete during version change).
    pub deleted: bool,
}

/// Metadata for an existing database.
#[derive(Debug, Clone, PartialEq)]
pub struct DatabaseMeta {
    /// Database name.
    pub name: Utf16String,
    /// Database version.
    pub version: u64,
    /// Object stores in this database.
    pub stores: Vec<StoreMeta>,
    /// Next store ID to assign.
    pub next_store_id: StoreId,
    /// Next index ID to assign.
    pub next_index_id: IndexId,
}

/// Cursor seek target.
#[derive(Debug, Clone, PartialEq)]
pub enum CursorSeek {
    /// Seek to the first record.
    First,
    /// Seek to a specific key.
    Key(Vec<u8>),
    /// Seek to a specific key and primary key.
    KeyAndPrimaryKey {
        /// The index key to seek to.
        key: Vec<u8>,
        /// The primary key to seek to.
        pkey: Vec<u8>,
    },
}
