//! In-memory storage implementation.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{Database, Storage};
use boa_idb_core::backend::types::DatabaseMeta;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{IndexId, StoreId};
use parking_lot::RwLock;

use crate::database::MemoryDatabase;

/// Key for a record: (store_id, encoded_key_bytes).
pub(crate) type RecordKey = (StoreId, Vec<u8>);

/// Key for an index entry: (index_id, index_key_bytes, primary_key_bytes).
pub(crate) type IndexKey = (IndexId, Vec<u8>, Vec<u8>);

/// Shared state for a memory storage.
#[derive(Debug, Default)]
pub(crate) struct StorageState {
    /// Database metadata.
    pub databases: HashMap<String, DatabaseMeta>,
    /// Primary records: (store_id, key_bytes) -> value_bytes.
    pub records: BTreeMap<RecordKey, Vec<u8>>,
    /// Index entries: (index_id, idx_key_bytes, primary_key_bytes) -> ().
    pub index_entries: BTreeMap<IndexKey, ()>,
    /// Key generators: store_id -> current_value.
    pub key_generators: HashMap<StoreId, f64>,
}

impl StorageState {
    /// Calculates approximate memory usage in bytes.
    pub fn usage_bytes(&self) -> u64 {
        let mut total: u64 = 0;
        // Database metadata overhead
        total += self.databases.len() as u64 * 256;
        // Record sizes
        for (k, v) in &self.records {
            total += (k.0.to_string().len() + k.1.len() + v.len()) as u64;
        }
        // Index entry sizes
        for (k, _) in &self.index_entries {
            total += (k.0.to_string().len() + k.1.len() + k.2.len()) as u64;
        }
        total
    }
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
        let mut state = self.state.write();
        let meta = state
            .databases
            .entry(name.to_string())
            .or_insert_with(|| DatabaseMeta {
                name: Utf16String::from(name),
                version: 0,
                stores: Vec::new(),
                next_store_id: 1,
                next_index_id: 1,
            })
            .clone();
        Ok(Box::new(MemoryDatabase::new(meta, self.state.clone())))
    }

    fn delete_database(&self, name: &str) -> Result<(), BackendError> {
        let mut state = self.state.write();
        if let Some(meta) = state.databases.remove(name) {
            // Clean up all records and index entries for this database
            let store_ids: Vec<StoreId> = meta.stores.iter().map(|s| s.id).collect();
            let index_ids: Vec<IndexId> = meta
                .stores
                .iter()
                .flat_map(|s| s.indexes.iter().map(|i| i.id))
                .collect();

            // Remove records
            for store_id in &store_ids {
                state.records.retain(|(sid, _), _| sid != store_id);
            }

            // Remove index entries
            for index_id in &index_ids {
                state.index_entries.retain(|(iid, _, _), _| iid != index_id);
            }

            // Remove key generators
            for store_id in &store_ids {
                state.key_generators.remove(store_id);
            }
        }
        Ok(())
    }

    fn usage_bytes(&self) -> Result<u64, BackendError> {
        let state = self.state.read();
        Ok(state.usage_bytes())
    }
}
