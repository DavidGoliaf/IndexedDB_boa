//! In-memory transaction with undo log support.

use std::collections::BTreeMap;
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendCursor, BackendTxn};
use boa_idb_core::backend::types::{DatabaseMeta, IndexSpec, StoreSpec};
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, IndexId, SourceRef, StoreId, TxnMode};
use parking_lot::RwLock;

use crate::cursor::MemoryCursor;
use crate::storage::StorageState;

/// Undo operation for savepoint rollback.
#[derive(Debug)]
enum UndoOp {
    /// Restore a record to its previous value (or delete if None).
    RestoreRecord {
        store: StoreId,
        key: Vec<u8>,
        old_value: Option<Vec<u8>>,
    },
    /// Restore an index entry.
    RestoreIndex {
        index: IndexId,
        idx_key: Vec<u8>,
        pkey: Vec<u8>,
        existed: bool,
    },
    /// Restore key generator value.
    RestoreKeyGen { store: StoreId, old_val: f64 },
}

/// In-memory transaction implementation.
pub struct MemoryTxn<'a> {
    mode: TxnMode,
    scope: Vec<StoreId>,
    meta: &'a mut DatabaseMeta,
    storage_state: Arc<RwLock<StorageState>>,

    // Transaction-local data
    records: BTreeMap<(StoreId, Vec<u8>), Option<Vec<u8>>>,
    index_records: BTreeMap<(IndexId, Vec<u8>, Vec<u8>), bool>,
    key_generators: BTreeMap<StoreId, f64>,

    // Undo log stack (one per savepoint level)
    undo_stack: Vec<Vec<UndoOp>>,

    // Schema changes (VersionChange only)
    new_stores: Vec<StoreSpec>,
    deleted_stores: Vec<StoreId>,
    new_indexes: Vec<(StoreId, IndexSpec)>,
    deleted_indexes: Vec<(StoreId, IndexId)>,
}

impl<'a> MemoryTxn<'a> {
    /// Creates a new memory transaction.
    pub fn new(
        mode: TxnMode,
        scope: Vec<StoreId>,
        meta: &'a mut DatabaseMeta,
        storage_state: Arc<RwLock<StorageState>>,
    ) -> Self {
        Self {
            mode,
            scope,
            meta,
            storage_state,
            records: BTreeMap::new(),
            index_records: BTreeMap::new(),
            key_generators: BTreeMap::new(),
            undo_stack: Vec::new(),
            new_stores: Vec::new(),
            deleted_stores: Vec::new(),
            new_indexes: Vec::new(),
            deleted_indexes: Vec::new(),
        }
    }

    fn check_scope(&self, store: StoreId) -> Result<(), BackendError> {
        if !self.scope.contains(&store) {
            return Err(BackendError::Internal(format!(
                "Store {store} not in transaction scope"
            )));
        }
        Ok(())
    }

    fn check_readwrite(&self) -> Result<(), BackendError> {
        if self.mode == TxnMode::ReadOnly {
            return Err(BackendError::Internal(
                "Cannot write in a read-only transaction".into(),
            ));
        }
        Ok(())
    }
}

impl<'a> BackendTxn for MemoryTxn<'a> {
    fn begin_request(&mut self) -> Result<(), BackendError> {
        self.undo_stack.push(Vec::new());
        Ok(())
    }

    fn commit_request(&mut self) -> Result<(), BackendError> {
        if let Some(ops) = self.undo_stack.pop() {
            // Merge into previous level if exists
            if let Some(top) = self.undo_stack.last_mut() {
                top.extend(ops);
            }
        }
        Ok(())
    }

    fn rollback_request(&mut self) -> Result<(), BackendError> {
        if let Some(ops) = self.undo_stack.pop() {
            for op in ops.into_iter().rev() {
                match op {
                    UndoOp::RestoreRecord {
                        store,
                        key,
                        old_value,
                    } => {
                        let map_key = (store, key);
                        match old_value {
                            Some(val) => {
                                self.records.insert(map_key, Some(val));
                            }
                            None => {
                                self.records.remove(&map_key);
                            }
                        }
                    }
                    UndoOp::RestoreIndex {
                        index,
                        idx_key,
                        pkey,
                        existed,
                    } => {
                        let map_key = (index, idx_key, pkey);
                        if existed {
                            self.index_records.insert(map_key, true);
                        } else {
                            self.index_records.remove(&map_key);
                        }
                    }
                    UndoOp::RestoreKeyGen { store, old_val } => {
                        self.key_generators.insert(store, old_val);
                    }
                }
            }
        }
        Ok(())
    }

    fn set_version(&mut self, version: u64) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "set_version requires VersionChange mode".into(),
            ));
        }
        self.meta.version = version;
        Ok(())
    }

    fn create_store(&mut self, spec: &StoreSpec) -> Result<StoreId, BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "create_store requires VersionChange mode".into(),
            ));
        }
        let id = self.meta.next_store_id;
        self.meta.next_store_id += 1;
        self.new_stores.push(spec.clone());
        Ok(id)
    }

    fn delete_store(&mut self, id: StoreId) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "delete_store requires VersionChange mode".into(),
            ));
        }
        self.deleted_stores.push(id);
        Ok(())
    }

    fn rename_store(&mut self, _id: StoreId, _new_name: &str) -> Result<(), BackendError> {
        // TODO: implement rename
        Ok(())
    }

    fn create_index(&mut self, store: StoreId, spec: &IndexSpec) -> Result<IndexId, BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "create_index requires VersionChange mode".into(),
            ));
        }
        let id = self.meta.next_index_id;
        self.meta.next_index_id += 1;
        self.new_indexes.push((store, spec.clone()));
        Ok(id)
    }

    fn delete_index(&mut self, store: StoreId, id: IndexId) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "delete_index requires VersionChange mode".into(),
            ));
        }
        self.deleted_indexes.push((store, id));
        Ok(())
    }

    fn rename_index(
        &mut self,
        _store: StoreId,
        _id: IndexId,
        _new_name: &str,
    ) -> Result<(), BackendError> {
        // TODO: implement rename
        Ok(())
    }

    fn put(
        &mut self,
        store: StoreId,
        key: &[u8],
        value: &[u8],
        _no_overwrite: bool,
    ) -> Result<(), BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        let map_key = (store, key.to_vec());

        // Save old value for undo
        let old_value = self.records.get(&map_key).and_then(|v| v.clone());
        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreRecord {
                store,
                key: key.to_vec(),
                old_value,
            });
        }

        self.records.insert(map_key, Some(value.to_vec()));
        Ok(())
    }

    fn get(&mut self, store: StoreId, key: &[u8]) -> Result<Option<Vec<u8>>, BackendError> {
        self.check_scope(store)?;

        let map_key = (store, key.to_vec());

        // Check transaction-local changes first
        if let Some(val) = self.records.get(&map_key) {
            return Ok(val.clone());
        }

        // Fall back to committed data
        let _state = self.storage_state.read();
        // TODO: implement committed data lookup
        Ok(None)
    }

    fn delete_record(&mut self, store: StoreId, key: &[u8]) -> Result<bool, BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        let map_key = (store, key.to_vec());

        // Save old value for undo
        let old_value = self.records.get(&map_key).and_then(|v| v.clone());
        let existed = old_value.is_some();

        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreRecord {
                store,
                key: key.to_vec(),
                old_value,
            });
        }

        self.records.insert(map_key, None);
        Ok(existed)
    }

    fn delete_range(&mut self, store: StoreId, _range: &EncodedRange) -> Result<u64, BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // TODO: implement range deletion
        Ok(0)
    }

    fn clear(&mut self, store: StoreId) -> Result<(), BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // TODO: implement clear
        Ok(())
    }

    fn count(&mut self, _src: SourceRef, _range: &EncodedRange) -> Result<u64, BackendError> {
        // TODO: implement count
        Ok(0)
    }

    fn scan(
        &mut self,
        _src: SourceRef,
        _range: &EncodedRange,
        _dir: Direction,
        _key_only: bool,
    ) -> Result<Box<dyn BackendCursor + '_>, BackendError> {
        // TODO: implement scan
        Ok(Box::new(MemoryCursor::new()))
    }

    fn key_gen_current(&self, store: StoreId) -> Result<f64, BackendError> {
        Ok(self.key_generators.get(&store).copied().unwrap_or(1.0))
    }

    fn key_gen_set(&mut self, store: StoreId, value: f64) -> Result<(), BackendError> {
        let old_val = self.key_generators.get(&store).copied().unwrap_or(1.0);

        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreKeyGen { store, old_val });
        }

        self.key_generators.insert(store, value);
        Ok(())
    }

    fn index_put(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
        _unique: bool,
    ) -> Result<(), BackendError> {
        let map_key = (index, idx_key.to_vec(), primary_key.to_vec());
        let existed = self.index_records.contains_key(&map_key);

        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreIndex {
                index,
                idx_key: idx_key.to_vec(),
                pkey: primary_key.to_vec(),
                existed,
            });
        }

        self.index_records.insert(map_key, true);
        Ok(())
    }

    fn index_delete(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
    ) -> Result<(), BackendError> {
        let map_key = (index, idx_key.to_vec(), primary_key.to_vec());
        let existed = self.index_records.contains_key(&map_key);

        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreIndex {
                index,
                idx_key: idx_key.to_vec(),
                pkey: primary_key.to_vec(),
                existed,
            });
        }

        self.index_records.remove(&map_key);
        Ok(())
    }

    fn index_delete_by_primary(
        &mut self,
        _index: IndexId,
        primary_key: &[u8],
    ) -> Result<(), BackendError> {
        // Find all index entries with this primary key
        let keys: Vec<_> = self
            .index_records
            .keys()
            .filter(|(_, _, pk)| pk == primary_key)
            .cloned()
            .collect();

        for key in keys {
            self.index_records.remove(&key);
        }

        Ok(())
    }

    fn commit(self: Box<Self>) -> Result<(), BackendError> {
        // Apply changes to shared state
        let _state = self.storage_state.write();

        // Apply record changes
        for (_store, _key) in self.records.keys() {
            // TODO: apply to committed storage
        }

        // Apply schema changes
        // TODO: apply schema changes

        Ok(())
    }

    fn abort(self: Box<Self>) -> Result<(), BackendError> {
        // Discard all changes
        Ok(())
    }
}
