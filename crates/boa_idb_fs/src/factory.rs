//! Factory for the filesystem IndexedDB backend.

use std::path::PathBuf;
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendFactory, Storage};
use boa_idb_core::proto::StorageKey;

use crate::naming::storage_root;
use crate::storage::FsStorage;
use crate::sync_hooks::{OsSyncHooks, SyncHooks};

/// Default `max_keys_in_memory` (R8.3.5).
pub const DEFAULT_MAX_KEYS_IN_MEMORY: u64 = 5_000_000;

/// Filesystem backend factory.
pub struct FsBackendFactory {
    root: PathBuf,
    hooks: Arc<dyn SyncHooks>,
    max_keys_in_memory: u64,
}

impl FsBackendFactory {
    /// Creates a factory rooted at `root` with OS sync hooks.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            hooks: Arc::new(OsSyncHooks),
            max_keys_in_memory: DEFAULT_MAX_KEYS_IN_MEMORY,
        }
    }

    /// Overrides sync hooks (tests inject counting observers).
    pub fn with_sync_hooks(mut self, hooks: Arc<dyn SyncHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Overrides the in-memory key limit.
    pub fn with_max_keys_in_memory(mut self, max: u64) -> Self {
        self.max_keys_in_memory = max;
        self
    }
}

impl BackendFactory for FsBackendFactory {
    fn open_storage(&self, key: &StorageKey) -> Result<Box<dyn Storage>, BackendError> {
        let path = storage_root(&self.root, &key.0);
        Ok(Box::new(FsStorage::new(
            path,
            key.0.clone(),
            self.hooks.clone(),
            self.max_keys_in_memory,
        )?))
    }
}
