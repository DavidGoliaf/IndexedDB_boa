//! In-memory transaction with undo log support.

use std::collections::BTreeMap;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendCursor, BackendTxn};
use boa_idb_core::backend::types::{DatabaseMeta, IndexSpec, StoreSpec};
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, IndexId, SourceRef, StoreId, TxnMode};
use parking_lot::RwLock;
use std::sync::Arc;

use crate::cursor::MemoryCursor;
use crate::storage::{IndexKey, RecordKey, StorageState};

/// Undo operation for savepoint rollback.
///
/// Record and index entries capture the *pending* state, not the merged read
/// view. A pending tombstone and a missing entry both read as "absent", but
/// rolling back to them differs: the former must re-install the tombstone
/// (the record was deleted by an outer level), the latter must remove the
/// entry (falling through to committed state). Logging merged values
/// resurrects records on nested rollback.
#[derive(Debug)]
enum UndoOp {
    /// Restore a pending record entry to its previous state.
    RestoreRecord { key: RecordKey, old: PendingSlot },
    /// Restore a pending index entry to its previous state.
    RestoreIndex { key: IndexKey, old: Option<bool> },
    /// Restore key generator value.
    RestoreKeyGen { store: StoreId, old_val: f64 },
}

/// Previous pending state of a record entry.
///
/// Three states because "reads as absent" is ambiguous: `Absent` means no
/// pending entry (reads fall through to committed storage), while `Deleted`
/// means a pending tombstone shadows the committed value.
#[derive(Debug, Clone)]
enum PendingSlot {
    /// No pending entry.
    Absent,
    /// Pending tombstone (deleted in this transaction).
    Deleted,
    /// Pending value.
    Value(Vec<u8>),
}

/// In-memory transaction implementation.
pub struct MemoryTxn {
    mode: TxnMode,
    scope: Vec<StoreId>,
    meta: DatabaseMeta,
    storage_state: Arc<RwLock<StorageState>>,

    // Transaction-local data (pending changes)
    pending_records: BTreeMap<RecordKey, Option<Vec<u8>>>,
    pending_index_entries: BTreeMap<IndexKey, bool>,
    pending_key_generators: BTreeMap<StoreId, f64>,

    // Undo log stack (one per savepoint level)
    undo_stack: Vec<Vec<UndoOp>>,
}

impl MemoryTxn {
    /// Creates a new memory transaction.
    pub fn new(
        mode: TxnMode,
        scope: Vec<StoreId>,
        meta: DatabaseMeta,
        storage_state: Arc<RwLock<StorageState>>,
    ) -> Self {
        Self {
            mode,
            scope,
            meta,
            storage_state,
            pending_records: BTreeMap::new(),
            pending_index_entries: BTreeMap::new(),
            pending_key_generators: BTreeMap::new(),
            undo_stack: Vec::new(),
        }
    }

    fn check_scope(&self, store: StoreId) -> Result<(), BackendError> {
        // Versionchange transactions span the whole database (exclusive mode,
        // stores come and go during the upgrade), so no scope check applies.
        if self.mode == TxnMode::VersionChange {
            return Ok(());
        }
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

    /// Reads a record, checking pending changes first, then committed storage.
    fn read_record(&self, key: &RecordKey) -> Option<Vec<u8>> {
        // Check pending changes
        if let Some(val) = self.pending_records.get(key) {
            return val.clone();
        }
        // Fall back to committed data
        let state = self.storage_state.read();
        state.records.get(key).cloned()
    }

    /// Captures the current pending slot of a record for the undo log.
    fn pending_slot(&self, key: &RecordKey) -> PendingSlot {
        match self.pending_records.get(key) {
            None => PendingSlot::Absent,
            Some(None) => PendingSlot::Deleted,
            Some(Some(val)) => PendingSlot::Value(val.clone()),
        }
    }

    /// Checks if a record exists (pending or committed).
    fn record_exists(&self, key: &RecordKey) -> bool {
        if let Some(val) = self.pending_records.get(key) {
            return val.is_some();
        }
        let state = self.storage_state.read();
        state.records.contains_key(key)
    }

    /// Checks if an index entry exists (pending or committed).
    fn index_entry_exists(&self, key: &IndexKey) -> bool {
        if let Some(existed) = self.pending_index_entries.get(key) {
            return *existed;
        }
        let state = self.storage_state.read();
        state.index_entries.contains_key(key)
    }

    /// Finds index entries matching a prefix (index_id, idx_key).
    fn find_index_entries(&self, index_id: IndexId, idx_key: &[u8]) -> Vec<(Vec<u8>, bool)> {
        let state = self.storage_state.read();
        let mut results = Vec::new();

        // Check committed entries
        for ((iid, ik, pk), _) in &state.index_entries {
            if *iid == index_id && ik == idx_key {
                let pending_key = (index_id, ik.clone(), pk.clone());
                // Check if overridden by pending
                if let Some(existed) = self.pending_index_entries.get(&pending_key) {
                    if *existed {
                        results.push((pk.clone(), true));
                    }
                } else {
                    results.push((pk.clone(), true));
                }
            }
        }

        // Check pending-only entries
        for ((iid, ik, pk), existed) in &self.pending_index_entries {
            if *iid == index_id && ik == idx_key && *existed {
                // Only add if not already in results
                if !results.iter().any(|(rpk, _)| rpk == pk) {
                    results.push((pk.clone(), true));
                }
            }
        }

        results
    }

    /// Finds all index entries for a primary key across all indexes.
    fn find_index_entries_by_primary(&self, index_id: IndexId, primary_key: &[u8]) -> Vec<Vec<u8>> {
        let state = self.storage_state.read();
        let mut results = Vec::new();

        // Check committed entries
        for ((iid, ik, pk), _) in &state.index_entries {
            if *iid == index_id && pk == primary_key {
                let pending_key = (index_id, ik.clone(), pk.clone());
                if let Some(existed) = self.pending_index_entries.get(&pending_key) {
                    if *existed {
                        results.push(ik.clone());
                    }
                } else {
                    results.push(ik.clone());
                }
            }
        }

        // Check pending-only entries (inserted in this transaction, not yet
        // committed, hence invisible to the loop above).
        for ((iid, ik, pk), existed) in &self.pending_index_entries {
            if *iid == index_id && pk.as_slice() == primary_key && *existed {
                if !results.contains(ik) {
                    results.push(ik.clone());
                }
            }
        }

        results
    }

    /// Collects index-scan entries over the merged pending/committed view.
    ///
    /// Entries are ordered ascending by `(index key, primary key)`; `*unique`
    /// directions collapse each index-key group to its first entry (smallest
    /// primary key), and `prev*` directions reverse the result.
    fn scan_index(
        &self,
        store: StoreId,
        index: IndexId,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
    ) -> Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> {
        // Merged (index_key, primary_key) set: committed entries minus
        // pending deletions, plus pending-only inserts.
        let state = self.storage_state.read();
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for ((iid, idx_key, pk), _) in &state.index_entries {
            if *iid != index || !range.contains(idx_key) {
                continue;
            }
            let key = (index, idx_key.clone(), pk.clone());
            if self
                .pending_index_entries
                .get(&key)
                .copied()
                .unwrap_or(true)
            {
                pairs.push((idx_key.clone(), pk.clone()));
            }
        }
        for ((iid, idx_key, pk), existed) in &self.pending_index_entries {
            if *iid != index || !*existed || !range.contains(idx_key) {
                continue;
            }
            let key = (index, idx_key.clone(), pk.clone());
            if !state.index_entries.contains_key(&key)
                && !pairs.contains(&(idx_key.clone(), pk.clone()))
            {
                pairs.push((idx_key.clone(), pk.clone()));
            }
        }

        // Resolve record values through the merged record view so index
        // cursors (non-key-only) yield values (read-your-writes).
        let mut entries: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> = Vec::with_capacity(pairs.len());
        for (idx_key, pk) in pairs {
            let value = if key_only {
                None
            } else {
                self.read_record(&(store, pk.clone()))
            };
            entries.push((idx_key, pk, value));
        }
        drop(state);

        // Sort by (index key, primary key) ascending.
        entries.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        // `*unique` directions yield one entry per index key — the first in
        // sort order, i.e. the one with the smallest primary key. Reversing
        // afterwards gives `prevunique` its groups in descending index-key
        // order with the same representative.
        if dir.is_unique() {
            let mut deduped: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> =
                Vec::with_capacity(entries.len());
            for entry in entries {
                let same_group = deduped
                    .last()
                    .is_some_and(|last: &(Vec<u8>, Vec<u8>, Option<Vec<u8>>)| last.0 == entry.0);
                if !same_group {
                    deduped.push(entry);
                }
            }
            entries = deduped;
        }
        if dir.is_prev() {
            entries.reverse();
        }

        entries
    }
}

impl BackendTxn for MemoryTxn {
    fn begin_request(&mut self) -> Result<(), BackendError> {
        self.undo_stack.push(Vec::new());
        Ok(())
    }

    fn commit_request(&mut self) -> Result<(), BackendError> {
        let Some(ops) = self.undo_stack.pop() else {
            return Err(BackendError::Internal(
                "commit_request without matching begin_request".into(),
            ));
        };
        // Merge into the parent level instead of discarding: an outer
        // rollback must still undo everything, including committed inner
        // requests (nested-savepoint semantics, AD-7).
        if let Some(parent) = self.undo_stack.last_mut() {
            parent.extend(ops);
        }
        Ok(())
    }

    fn rollback_request(&mut self) -> Result<(), BackendError> {
        let Some(ops) = self.undo_stack.pop() else {
            return Err(BackendError::Internal(
                "rollback_request without matching begin_request".into(),
            ));
        };
        for op in ops.into_iter().rev() {
            match op {
                UndoOp::RestoreRecord { key, old } => match old {
                    PendingSlot::Value(val) => {
                        self.pending_records.insert(key, Some(val));
                    }
                    PendingSlot::Deleted => {
                        self.pending_records.insert(key, None);
                    }
                    PendingSlot::Absent => {
                        self.pending_records.remove(&key);
                    }
                },
                UndoOp::RestoreIndex { key, old } => match old {
                    Some(existed) => {
                        self.pending_index_entries.insert(key, existed);
                    }
                    None => {
                        self.pending_index_entries.remove(&key);
                    }
                },
                UndoOp::RestoreKeyGen { store, old_val } => {
                    self.pending_key_generators.insert(store, old_val);
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
        self.meta
            .stores
            .push(boa_idb_core::backend::types::StoreMeta {
                id,
                name: spec.name.clone(),
                key_path: spec.key_path.clone(),
                auto_increment: spec.auto_increment,
                key_gen: 1.0,
                indexes: Vec::new(),
                deleted: false,
            });
        Ok(id)
    }

    fn delete_store(&mut self, id: StoreId) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "delete_store requires VersionChange mode".into(),
            ));
        }
        if let Some(store) = self.meta.stores.iter_mut().find(|s| s.id == id) {
            store.deleted = true;
        }
        Ok(())
    }

    fn rename_store(&mut self, id: StoreId, new_name: &str) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "rename_store requires VersionChange mode".into(),
            ));
        }
        if let Some(store) = self.meta.stores.iter_mut().find(|s| s.id == id) {
            store.name = new_name.into();
        }
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
        if let Some(s) = self.meta.stores.iter_mut().find(|s| s.id == store) {
            s.indexes.push(boa_idb_core::backend::types::IndexMeta {
                id,
                store_id: store,
                name: spec.name.clone(),
                key_path: spec.key_path.clone(),
                unique: spec.unique,
                multi_entry: spec.multi_entry,
                deleted: false,
            });
        }
        Ok(id)
    }

    fn delete_index(&mut self, store: StoreId, id: IndexId) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "delete_index requires VersionChange mode".into(),
            ));
        }
        if let Some(s) = self.meta.stores.iter_mut().find(|s| s.id == store) {
            if let Some(idx) = s.indexes.iter_mut().find(|i| i.id == id) {
                idx.deleted = true;
            }
        }
        Ok(())
    }

    fn rename_index(
        &mut self,
        store: StoreId,
        id: IndexId,
        new_name: &str,
    ) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "rename_index requires VersionChange mode".into(),
            ));
        }
        if let Some(s) = self.meta.stores.iter_mut().find(|s| s.id == store) {
            if let Some(idx) = s.indexes.iter_mut().find(|i| i.id == id) {
                idx.name = new_name.into();
            }
        }
        Ok(())
    }

    fn put(
        &mut self,
        store: StoreId,
        key: &[u8],
        value: &[u8],
        no_overwrite: bool,
    ) -> Result<(), BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        let map_key = (store, key.to_vec());

        // Check no_overwrite
        if no_overwrite && self.record_exists(&map_key) {
            return Err(BackendError::Constraint(format!(
                "Record already exists for key in store {store}"
            )));
        }

        // Save pending state for undo (Absent = no pending entry yet).
        let old = self.pending_slot(&map_key);
        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreRecord {
                key: map_key.clone(),
                old,
            });
        }

        self.pending_records.insert(map_key, Some(value.to_vec()));
        Ok(())
    }

    fn get(&mut self, store: StoreId, key: &[u8]) -> Result<Option<Vec<u8>>, BackendError> {
        self.check_scope(store)?;
        let map_key = (store, key.to_vec());
        Ok(self.read_record(&map_key))
    }

    fn delete_record(&mut self, store: StoreId, key: &[u8]) -> Result<bool, BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        let map_key = (store, key.to_vec());
        let old_value = self.read_record(&map_key);
        let existed = old_value.is_some();

        let old = self.pending_slot(&map_key);
        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreRecord {
                key: map_key.clone(),
                old,
            });
        }

        self.pending_records.insert(map_key, None);
        Ok(existed)
    }

    fn delete_range(&mut self, store: StoreId, range: &EncodedRange) -> Result<u64, BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // Collect keys in range from committed storage
        let state = self.storage_state.read();
        let keys_in_range: Vec<RecordKey> = state
            .records
            .keys()
            .filter(|(sid, key)| *sid == store && range.contains(key))
            .cloned()
            .collect();
        drop(state);

        // Also collect pending-only keys in range
        let pending_keys: Vec<RecordKey> = self
            .pending_records
            .keys()
            .filter(|(sid, key)| *sid == store && range.contains(key))
            .cloned()
            .collect();

        let mut count: u64 = 0;

        // Delete committed records
        for key in &keys_in_range {
            let old_value = self.read_record(key);
            if old_value.is_some() {
                count += 1;
            }
            let old = self.pending_slot(key);
            if let Some(ops) = self.undo_stack.last_mut() {
                ops.push(UndoOp::RestoreRecord {
                    key: key.clone(),
                    old,
                });
            }
            self.pending_records.insert(key.clone(), None);
        }

        // Delete pending-only records (not already handled).
        //
        // The previous pending value is logged for undo so a rollback
        // restores it; only records that actually exist are counted.
        for key in pending_keys {
            if keys_in_range.contains(&key) {
                continue;
            }
            if let Some(Some(val)) = self.pending_records.get(&key).cloned() {
                if let Some(ops) = self.undo_stack.last_mut() {
                    ops.push(UndoOp::RestoreRecord {
                        key: key.clone(),
                        old: PendingSlot::Value(val),
                    });
                }
                self.pending_records.remove(&key);
                count += 1;
            }
            // Tombstone or vanished entry: no live record, no count.
        }

        Ok(count)
    }

    fn clear(&mut self, store: StoreId) -> Result<(), BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // Union of committed and pending keys for this store. The undo entry
        // for each key captures the *merged* value (including outer-level
        // tombstones) — logging the raw committed value here would resurrect
        // records deleted by an outer savepoint level on rollback.
        let state = self.storage_state.read();
        let mut keys: Vec<RecordKey> = state
            .records
            .keys()
            .filter(|(sid, _)| *sid == store)
            .cloned()
            .collect();
        drop(state);
        for key in self.pending_records.keys().filter(|(sid, _)| *sid == store) {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }

        for key in keys {
            let old = self.pending_slot(&key);
            if let Some(ops) = self.undo_stack.last_mut() {
                ops.push(UndoOp::RestoreRecord {
                    key: key.clone(),
                    old,
                });
            }
            // Tombstone (not removal): keeps the merged view consistent for
            // any further reads in this transaction.
            self.pending_records.insert(key, None);
        }

        Ok(())
    }

    fn count(&mut self, src: SourceRef, range: &EncodedRange) -> Result<u64, BackendError> {
        match src {
            SourceRef::Store(store) => {
                self.check_scope(store)?;
                let mut count: u64 = 0;

                // Count committed records in range
                let state = self.storage_state.read();
                for (sid, key) in state.records.keys() {
                    if *sid == store && range.contains(key) {
                        // Check if not deleted in pending
                        let pending_key = (store, key.clone());
                        if let Some(val) = self.pending_records.get(&pending_key) {
                            if val.is_some() {
                                count += 1;
                            }
                        } else {
                            count += 1;
                        }
                    }
                }
                drop(state);

                // Count pending-only records in range
                for ((sid, key), val) in &self.pending_records {
                    if *sid == store && val.is_some() && range.contains(key) {
                        // Only count if not already counted from committed
                        let committed_exists = {
                            let state = self.storage_state.read();
                            state.records.contains_key(&(*sid, key.clone()))
                        };
                        if !committed_exists {
                            count += 1;
                        }
                    }
                }

                Ok(count)
            }
            SourceRef::Index { store, index } => {
                self.check_scope(store)?;
                let mut count: u64 = 0;

                // Merged view: committed entries, minus those deleted in
                // pending, plus pending-only inserts.
                let state = self.storage_state.read();
                for ((iid, idx_key, pk), _) in &state.index_entries {
                    if *iid != index || !range.contains(idx_key) {
                        continue;
                    }
                    let key = (index, idx_key.clone(), pk.clone());
                    if self
                        .pending_index_entries
                        .get(&key)
                        .copied()
                        .unwrap_or(true)
                    {
                        count += 1;
                    }
                }
                for ((iid, idx_key, pk), existed) in &self.pending_index_entries {
                    if *iid != index || !*existed || !range.contains(idx_key) {
                        continue;
                    }
                    let key = (index, idx_key.clone(), pk.clone());
                    if !state.index_entries.contains_key(&key) {
                        count += 1;
                    }
                }
                drop(state);

                Ok(count)
            }
        }
    }

    fn scan(
        &mut self,
        src: SourceRef,
        range: &EncodedRange,
        dir: Direction,
        key_only: bool,
    ) -> Result<Box<dyn BackendCursor + '_>, BackendError> {
        match src {
            SourceRef::Store(store) => {
                self.check_scope(store)?;

                // Collect merged records in range
                let mut entries: Vec<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> = Vec::new();

                // Add committed records
                let state = self.storage_state.read();
                for ((sid, key), value) in &state.records {
                    if *sid == store && range.contains(key) {
                        // Check if overridden by pending
                        let pending_key = (store, key.clone());
                        if let Some(pending_val) = self.pending_records.get(&pending_key) {
                            if let Some(v) = pending_val {
                                entries.push((key.clone(), key.clone(), Some(v.clone())));
                            }
                            // If None, record is deleted - skip
                        } else {
                            entries.push((key.clone(), key.clone(), Some(value.clone())));
                        }
                    }
                }
                drop(state);

                // Add pending-only records
                for ((sid, key), val) in &self.pending_records {
                    if *sid == store && val.is_some() && range.contains(key) {
                        // Only add if not already added from committed
                        let committed_exists = {
                            let state = self.storage_state.read();
                            state.records.contains_key(&(*sid, key.clone()))
                        };
                        if !committed_exists {
                            entries.push((key.clone(), key.clone(), val.clone()));
                        }
                    }
                }

                // Sort by key
                entries.sort_by(|a, b| a.0.cmp(&b.0));

                // Apply direction
                let cursor = match dir {
                    Direction::Next | Direction::NextUnique => {
                        if key_only {
                            MemoryCursor::with_entries(
                                entries
                                    .into_iter()
                                    .map(|(k, pk, _)| (k, pk, None))
                                    .collect(),
                            )
                        } else {
                            MemoryCursor::with_entries(entries)
                        }
                    }
                    Direction::Prev | Direction::PrevUnique => {
                        if key_only {
                            MemoryCursor::with_entries_reversed(
                                entries
                                    .into_iter()
                                    .map(|(k, pk, _)| (k, pk, None))
                                    .collect(),
                            )
                        } else {
                            MemoryCursor::with_entries_reversed(entries)
                        }
                    }
                };

                Ok(Box::new(cursor))
            }
            SourceRef::Index { store, index } => {
                self.check_scope(store)?;
                let entries = self.scan_index(store, index, range, dir, key_only);
                Ok(Box::new(MemoryCursor::with_entries(entries)))
            }
        }
    }

    fn key_gen_current(&self, store: StoreId) -> Result<f64, BackendError> {
        // Check pending first
        if let Some(val) = self.pending_key_generators.get(&store) {
            return Ok(*val);
        }
        // Fall back to committed
        let state = self.storage_state.read();
        Ok(state.key_generators.get(&store).copied().unwrap_or(1.0))
    }

    fn key_gen_set(&mut self, store: StoreId, value: f64) -> Result<(), BackendError> {
        self.check_readwrite()?;
        let old_val = self.key_gen_current(store)?;

        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreKeyGen { store, old_val });
        }

        self.pending_key_generators.insert(store, value);
        Ok(())
    }

    fn index_put(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
        unique: bool,
    ) -> Result<(), BackendError> {
        self.check_readwrite()?;
        let map_key = (index, idx_key.to_vec(), primary_key.to_vec());
        let existed = self.index_entry_exists(&map_key);

        // Check unique constraint
        if unique && !existed {
            // Check if another entry with the same idx_key exists
            let existing = self.find_index_entries(index, idx_key);
            if !existing.is_empty() {
                return Err(BackendError::Constraint(format!(
                    "Unique constraint violation for index {index}"
                )));
            }
        }

        let old = self.pending_index_entries.get(&map_key).copied();
        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreIndex {
                key: map_key.clone(),
                old,
            });
        }

        self.pending_index_entries.insert(map_key, true);
        Ok(())
    }

    fn index_delete(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
    ) -> Result<(), BackendError> {
        self.check_readwrite()?;
        let map_key = (index, idx_key.to_vec(), primary_key.to_vec());
        let old = self.pending_index_entries.get(&map_key).copied();

        if let Some(ops) = self.undo_stack.last_mut() {
            ops.push(UndoOp::RestoreIndex {
                key: map_key.clone(),
                old,
            });
        }

        self.pending_index_entries.insert(map_key, false);
        Ok(())
    }

    fn index_delete_by_primary(
        &mut self,
        index: IndexId,
        primary_key: &[u8],
    ) -> Result<(), BackendError> {
        self.check_readwrite()?;
        // Find all index entries for this primary key in this specific index
        let idx_keys = self.find_index_entries_by_primary(index, primary_key);

        for idx_key in idx_keys {
            let map_key = (index, idx_key, primary_key.to_vec());
            let old = self.pending_index_entries.get(&map_key).copied();

            if let Some(ops) = self.undo_stack.last_mut() {
                ops.push(UndoOp::RestoreIndex {
                    key: map_key.clone(),
                    old,
                });
            }

            self.pending_index_entries.insert(map_key, false);
        }

        Ok(())
    }

    fn commit(self: Box<Self>) -> Result<(), BackendError> {
        let mut state = self.storage_state.write();

        // Apply pending record changes
        for (key, value) in &self.pending_records {
            match value {
                Some(val) => {
                    state.records.insert(key.clone(), val.clone());
                }
                None => {
                    state.records.remove(key);
                }
            }
        }

        // Apply pending index entry changes
        for (key, exists) in &self.pending_index_entries {
            if *exists {
                state.index_entries.insert(key.clone(), ());
            } else {
                state.index_entries.remove(key);
            }
        }

        // Apply pending key generator changes
        for (store, value) in &self.pending_key_generators {
            state.key_generators.insert(*store, *value);
        }

        // Apply schema changes
        state
            .databases
            .insert(self.meta.name.to_string(), self.meta);

        Ok(())
    }

    fn abort(self: Box<Self>) -> Result<(), BackendError> {
        // Discard all changes
        Ok(())
    }
}
