//! Core traits for storage backends.

use crate::backend::capabilities::BackendCapabilities;
use crate::backend::error::BackendError;
use crate::backend::types::{CursorSeek, DatabaseMeta, IndexSpec, StoreSpec};
use crate::key::range::EncodedRange;
use crate::proto::{Direction, Durability, IndexId, SourceRef, StorageKey, StoreId, TxnMode};

/// Factory for creating storage instances.
pub trait BackendFactory: Send + Sync + 'static {
    /// Opens a storage instance for the given storage key.
    fn open_storage(&self, key: &StorageKey) -> Result<Box<dyn Storage>, BackendError>;
}

/// A storage instance manages multiple databases.
pub trait Storage: Send + 'static {
    /// Lists all databases with their versions.
    fn list_databases(&self) -> Result<Vec<(String, u64)>, BackendError>;

    /// Opens a database by name.
    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError>;

    /// Deletes a database by name.
    fn delete_database(&self, name: &str) -> Result<(), BackendError>;

    /// Returns the storage usage in bytes.
    fn usage_bytes(&self) -> Result<u64, BackendError>;
}

/// A database instance.
pub trait Database: Send + 'static {
    /// Returns the database metadata.
    fn metadata(&self) -> &DatabaseMeta;

    /// Returns the backend capabilities.
    fn capabilities(&self) -> BackendCapabilities;

    /// Begins a new transaction.
    fn begin(
        &mut self,
        mode: TxnMode,
        scope: &[StoreId],
        durability: Durability,
    ) -> Result<Box<dyn BackendTxn + '_>, BackendError>;

    /// Flushes pending changes to durable storage.
    fn flush(&mut self) -> Result<(), BackendError>;

    /// Closes the database.
    fn close(self: Box<Self>) -> Result<(), BackendError>;
}

/// A backend transaction.
pub trait BackendTxn {
    // --- Savepoints (one savepoint per IDBRequest) ---
    /// Begins a new request savepoint.
    fn begin_request(&mut self) -> Result<(), BackendError>;

    /// Commits the current request savepoint.
    fn commit_request(&mut self) -> Result<(), BackendError>;

    /// Rolls back the current request savepoint.
    fn rollback_request(&mut self) -> Result<(), BackendError>;

    // --- Schema operations (VersionChange mode only) ---
    /// Sets the database version.
    fn set_version(&mut self, version: u64) -> Result<(), BackendError>;

    /// Creates a new object store.
    fn create_store(&mut self, spec: &StoreSpec) -> Result<StoreId, BackendError>;

    /// Deletes an object store.
    fn delete_store(&mut self, id: StoreId) -> Result<(), BackendError>;

    /// Renames an object store.
    fn rename_store(&mut self, id: StoreId, new_name: &str) -> Result<(), BackendError>;

    /// Creates a new index on a store.
    fn create_index(&mut self, store: StoreId, spec: &IndexSpec) -> Result<IndexId, BackendError>;

    /// Deletes an index.
    fn delete_index(&mut self, store: StoreId, id: IndexId) -> Result<(), BackendError>;

    /// Renames an index.
    fn rename_index(
        &mut self,
        store: StoreId,
        id: IndexId,
        new_name: &str,
    ) -> Result<(), BackendError>;

    // --- Data operations ---
    /// Stores a record.
    fn put(
        &mut self,
        store: StoreId,
        key: &[u8],
        value: &[u8],
        no_overwrite: bool,
    ) -> Result<(), BackendError>;

    /// Retrieves a record by key.
    fn get(&mut self, store: StoreId, key: &[u8]) -> Result<Option<Vec<u8>>, BackendError>;

    /// Deletes a single record by key. Returns true if the record existed.
    fn delete_record(&mut self, store: StoreId, key: &[u8]) -> Result<bool, BackendError>;

    /// Deletes records in a range. Returns the number of deleted records.
    fn delete_range(&mut self, store: StoreId, range: &EncodedRange) -> Result<u64, BackendError>;

    /// Clears all records from a store.
    fn clear(&mut self, store: StoreId) -> Result<(), BackendError>;

    /// Counts records matching a range.
    fn count(&mut self, src: SourceRef, range: &EncodedRange) -> Result<u64, BackendError>;

    /// Opens a cursor for scanning records.
    fn scan(
        &mut self,
        src: SourceRef,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
    ) -> Result<Box<dyn BackendCursor + '_>, BackendError>;

    // --- Key generator ---
    /// Returns the current key generator value for a store.
    fn key_gen_current(&self, store: StoreId) -> Result<f64, BackendError>;

    /// Sets the key generator value for a store.
    fn key_gen_set(&mut self, store: StoreId, value: f64) -> Result<(), BackendError>;

    // --- Index entries ---
    /// Inserts an index entry.
    fn index_put(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
        unique: bool,
    ) -> Result<(), BackendError>;

    /// Deletes an index entry.
    fn index_delete(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
    ) -> Result<(), BackendError>;

    /// Deletes all index entries for a primary key.
    fn index_delete_by_primary(
        &mut self,
        index: IndexId,
        primary_key: &[u8],
    ) -> Result<(), BackendError>;

    // --- Transaction completion ---
    /// Commits the transaction.
    fn commit(self: Box<Self>) -> Result<(), BackendError>;

    /// Aborts the transaction.
    fn abort(self: Box<Self>) -> Result<(), BackendError>;
}

/// A backend cursor for iterating over records.
pub trait BackendCursor {
    /// Seeks the cursor to a target position.
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError>;

    /// Steps the cursor forward by count positions.
    fn step(&mut self, count: u32) -> Result<bool, BackendError>;

    /// Returns the current key bytes.
    fn current_key(&self) -> &[u8];

    /// Returns the current primary key bytes.
    fn current_primary_key(&self) -> &[u8];

    /// Returns the current value bytes (None for key-only cursors).
    fn current_value(&self) -> Option<&[u8]>;
}
