//! In-memory storage implementation.

use std::collections::HashMap;
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{Database, Storage};
use boa_idb_core::backend::types::DatabaseMeta;
use boa_idb_core::key::utf16::Utf16String;
use parking_lot::RwLock;

use crate::database::MemoryDatabase;

/// Shared state for a memory storage.
#[derive(Debug, Default)]
pub(crate) struct StorageState {
    databases: HashMap<String, DatabaseMeta>,
}

/// In-memory storage implementation.
#[derive(Debug, Clone)]
pub struct MemoryStorage {
    state: Arc<RwLock<StorageState>>,
}

impl MemoryStorage {
    /// Creates a new empty memory storage.
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(StorageState::default())),
        }
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage for MemoryStorage {
    fn list_databases(&self) -> Result<Vec<(String, u64)>, BackendError> {
        let state = self.state.read();
        Ok(state
            .databases
            .iter()
            .map(|(name, meta)| (name.clone(), meta.version))
            .collect())
    }

    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError> {
        let state = self.state.read();
        let meta = state
            .databases
            .get(name)
            .cloned()
            .unwrap_or_else(|| DatabaseMeta {
                name: Utf16String::from(name),
                version: 0,
                stores: Vec::new(),
                next_store_id: 1,
                next_index_id: 1,
            });
        Ok(Box::new(MemoryDatabase::new(meta, self.state.clone())))
    }

    fn delete_database(&self, name: &str) -> Result<(), BackendError> {
        let mut state = self.state.write();
        state.databases.remove(name);
        Ok(())
    }

    fn usage_bytes(&self) -> Result<u64, BackendError> {
        // Rough estimate: count of databases * 1024
        let state = self.state.read();
        Ok(state.databases.len() as u64 * 1024)
    }
}
