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
///
/// One `StorageState` instance holds the data of exactly ONE database (see
/// [`MemoryStorage`]): store/index ids restart at 1 per database, so sharing
/// `records` across databases would collide unrelated stores.
#[derive(Debug, Default)]
pub(crate) struct StorageState {
    /// Database metadata (holds the owning database's entry).
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

    /// Removes every record, index entry and generator of the given stores.
    ///
    /// Used by [`MemoryStorage::delete_database`]: with per-database states
    /// this is only a fast path before the whole part is dropped.
    fn remove_stores(&mut self, store_ids: &[StoreId], index_ids: &[IndexId]) {
        for store_id in store_ids {
            self.records.retain(|(sid, _), _| sid != store_id);
        }
        for index_id in index_ids {
            self.index_entries.retain(|(iid, _, _), _| iid != index_id);
        }
        for store_id in store_ids {
            self.key_generators.remove(store_id);
        }
    }
}

/// In-memory storage implementation.
///
/// Databases are fully isolated: each database owns a separate
/// `StorageState` part, because store and index ids restart at 1 per
/// database and must never collide across databases sharing one storage.
#[derive(Debug, Clone, Default)]
pub struct MemoryStorage {
    parts: Arc<RwLock<HashMap<String, Arc<RwLock<StorageState>>>>>,
}

impl MemoryStorage {
    /// Creates a new empty memory storage.
    pub fn new() -> Self {
        Self {
            parts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Returns the isolated state part of a database, creating it on demand.
    fn part(&self, name: &str) -> Arc<RwLock<StorageState>> {
        let mut parts = self.parts.write();
        parts
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(StorageState::default())))
            .clone()
    }
}

impl Storage for MemoryStorage {
    fn list_databases(&self) -> Result<Vec<(String, u64)>, BackendError> {
        let parts = self.parts.read();
        let mut out = Vec::new();
        for part in parts.values() {
            let state = part.read();
            out.extend(
                state
                    .databases
                    .iter()
                    .map(|(name, meta)| (name.clone(), meta.version)),
            );
        }
        Ok(out)
    }

    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError> {
        let part = self.part(name);
        let meta = {
            let mut state = part.write();
            state
                .databases
                .entry(name.to_string())
                .or_insert_with(|| DatabaseMeta {
                    name: Utf16String::from(name),
                    version: 0,
                    stores: Vec::new(),
                    next_store_id: 1,
                    next_index_id: 1,
                })
                .clone()
        };
        Ok(Box::new(MemoryDatabase::new(meta, part)))
    }

    fn delete_database(&self, name: &str) -> Result<(), BackendError> {
        let removed = self.parts.write().remove(name);
        if let Some(part) = removed {
            // Belt and braces: scrub store-scoped data before dropping, so no
            // stale entry can survive through another Arc handle.
            let mut state = part.write();
            if let Some(meta) = state.databases.remove(name) {
                let store_ids: Vec<StoreId> = meta.stores.iter().map(|s| s.id).collect();
                let index_ids: Vec<IndexId> = meta
                    .stores
                    .iter()
                    .flat_map(|s| s.indexes.iter().map(|i| i.id))
                    .collect();
                state.remove_stores(&store_ids, &index_ids);
            }
        }
        Ok(())
    }

    fn usage_bytes(&self) -> Result<u64, BackendError> {
        let parts = self.parts.read();
        Ok(parts.values().map(|p| p.read().usage_bytes()).sum())
    }
}
