//! SQLite backend factory.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendFactory, Storage};
use boa_idb_core::proto::StorageKey;
use std::path::{Path, PathBuf};

use crate::naming;
use crate::storage::SqliteStorage;

/// Factory for creating SQLite-backed storage instances.
pub struct SqliteBackendFactory {
    root: PathBuf,
}

impl SqliteBackendFactory {
    /// Creates a new factory with the given root directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the on-disk directory for a storage key (hashed, R8.1.4).
    ///
    /// Storage keys are never used verbatim in filesystem paths.
    pub fn storage_dir(&self, key: &StorageKey) -> PathBuf {
        self.root.join(naming::storage_dir_name(&key.0))
    }
}

impl BackendFactory for SqliteBackendFactory {
    fn open_storage(&self, key: &StorageKey) -> Result<Box<dyn Storage>, BackendError> {
        let storage_dir = self.storage_dir(key);
        Ok(Box::new(SqliteStorage::open(&storage_dir, key)?))
    }
}
