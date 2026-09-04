//! In-memory ordered index rebuilt from meta + WAL (M6-A).

use boa_idb_core::backend::types::DatabaseMeta;
use boa_idb_core::proto::{IndexId, StoreId};
use std::collections::{BTreeMap, HashMap};

/// Key for a record: (store_id, encoded_key_bytes).
pub(crate) type RecordKey = (StoreId, Vec<u8>);

/// Key for an index entry: (index_id, index_key_bytes, primary_key_bytes).
pub(crate) type IndexKey = (IndexId, Vec<u8>, Vec<u8>);

/// Mutable database state held in memory while the DB is open.
#[derive(Debug, Clone, Default)]
pub(crate) struct DbState {
    /// Database metadata.
    pub meta: Option<DatabaseMeta>,
    /// Primary records.
    pub records: BTreeMap<RecordKey, Vec<u8>>,
    /// Index entries.
    pub index_entries: BTreeMap<IndexKey, ()>,
    /// Key generators.
    pub key_generators: HashMap<StoreId, f64>,
    /// Next WAL transaction sequence.
    pub next_txn_seq: u64,
    /// Manifest sequence counter.
    pub manifest_seq: u64,
}

impl DbState {
    /// Counts primary keys currently resident in memory.
    pub fn key_count(&self) -> u64 {
        self.records.len() as u64
    }

    /// Approximate usage estimate for `Storage::usage_bytes`.
    pub fn usage_bytes(&self) -> u64 {
        let mut total = 256u64;
        for (k, v) in &self.records {
            total += (8 + k.1.len() + v.len()) as u64;
        }
        for (k, _) in &self.index_entries {
            total += (8 + k.1.len() + k.2.len()) as u64;
        }
        total
    }
}
