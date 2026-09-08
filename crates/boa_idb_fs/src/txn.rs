//! Filesystem-backed transaction with undo logs and WAL commit.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendCursor, BackendTxn};
use boa_idb_core::backend::types::{DatabaseMeta, IndexSpec, StoreSpec};
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, Durability, IndexId, SourceRef, StoreId, TxnMode};
use parking_lot::RwLock;

use crate::atomic::{append_and_maybe_sync, truncate_file};
use crate::compact::{CompactConfig, maybe_compact, wal_path};
use crate::cursor::{CursorMaps, FsCursor};
use crate::meta::{encode_meta, write_meta_file};
use crate::state::{DbState, IndexKey, RecordKey, SnapshotMeter};
use crate::vfs::FileSystem;
use crate::wal::{CodecError, WalOp, encode_txn_frames_limited};

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

/// Filesystem transaction: pending mutations + WAL on commit.
pub struct FsTxn {
    mode: TxnMode,
    scope: Vec<StoreId>,
    meta: DatabaseMeta,
    state: Arc<RwLock<DbState>>,
    durability: Durability,
    db_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    max_keys_in_memory: u64,
    max_frame_payload: u32,
    compact: CompactConfig,
    #[allow(dead_code)]
    meter: Arc<SnapshotMeter>,
    schema_dirty: bool,

    // Transaction-local data (pending changes)
    pending_records: BTreeMap<RecordKey, Option<Vec<u8>>>,
    pending_index_entries: BTreeMap<IndexKey, bool>,
    pending_key_generators: BTreeMap<StoreId, f64>,

    // Undo log stack (one per savepoint level)
    undo_stack: Vec<Vec<UndoOp>>,

    /// Net pending record inserts vs committed storage (M7-B H-F1).
    ///
    /// Lets quota projection stay O(1) per mutation instead of rescanning
    /// all pending records per `put` (which was quadratic in batch size).
    key_delta: i64,
    /// `key_delta` snapshot per savepoint frame (parallel to `undo_stack`).
    key_delta_stack: Vec<i64>,
}

impl FsTxn {
    /// Creates a new filesystem transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mode: TxnMode,
        scope: Vec<StoreId>,
        meta: DatabaseMeta,
        state: Arc<RwLock<DbState>>,
        durability: Durability,
        db_dir: PathBuf,
        fs: Arc<dyn FileSystem>,
        max_keys_in_memory: u64,
        max_frame_payload: u32,
        compact: CompactConfig,
        meter: Arc<SnapshotMeter>,
    ) -> Self {
        Self {
            mode,
            scope,
            meta,
            state,
            durability,
            db_dir,
            fs,
            max_keys_in_memory,
            max_frame_payload,
            compact,
            meter,
            schema_dirty: false,
            pending_records: BTreeMap::new(),
            pending_index_entries: BTreeMap::new(),
            pending_key_generators: BTreeMap::new(),
            undo_stack: Vec::new(),
            key_delta: 0,
            key_delta_stack: Vec::new(),
        }
    }

    fn wants_sync(&self) -> bool {
        matches!(self.durability, Durability::Strict)
    }

    /// Adjusts the pending key delta for a record presence transition.
    ///
    /// Keeps quota projection O(1) per mutation (M7-B H-F1): the previous
    /// implementation rescanned all pending records on every `put`, making
    /// batches quadratic. `before` is the effective existence ahead of the
    /// mutation, `after` the existence it establishes. Takes only the delta
    /// (not `&mut self`) so callers holding the state read guard compile.
    fn apply_record_transition(delta: &mut i64, before: bool, after: bool) {
        match (before, after) {
            (false, true) => *delta += 1,
            (true, false) => *delta -= 1,
            _ => {}
        }
    }

    /// Projected key count after inserting one brand-new record.
    ///
    /// Fresh O(1) committed count plus the maintained pending delta: proven
    /// equivalent to the previous rescan for every key state (new key,
    /// overwrite, delete-then-put within one transaction), since the delta
    /// only tracks this transaction's own mutations. Callers gate on the
    /// key being absent (overwrites project no growth).
    fn projected_key_count_for_insert(&self) -> u64 {
        let committed = self.state.read().key_count();
        let projected = i128::from(committed) + i128::from(self.key_delta) + 1;
        u64::try_from(projected.max(0)).unwrap_or(u64::MAX)
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
        let state = self.state.read();
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
        let state = self.state.read();
        state.records.contains_key(key)
    }

    /// Checks if an index entry exists (pending or committed).
    fn index_entry_exists(&self, key: &IndexKey) -> bool {
        if let Some(existed) = self.pending_index_entries.get(key) {
            return *existed;
        }
        let state = self.state.read();
        state.index_entries.contains_key(key)
    }

    /// Finds index entries matching a prefix (index_id, idx_key).
    fn find_index_entries(&self, index_id: IndexId, idx_key: &[u8]) -> Vec<(Vec<u8>, bool)> {
        let state = self.state.read();
        let mut results = Vec::new();

        // Check committed entries
        for (ikey, _) in state.index_entries.iter() {
            let (iid, ik, pk) = ikey;
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
        let state = self.state.read();
        let mut results = Vec::new();

        // Check committed entries
        for (ikey, _) in state.index_entries.iter() {
            let (iid, ik, pk) = ikey;
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

    fn build_commit_payload(&self) -> (Vec<WalOp>, DatabaseMeta, bool) {
        let mut ops = Vec::new();
        for (key, value) in &self.pending_records {
            match value {
                Some(val) => ops.push(WalOp::Put {
                    store: key.0,
                    key: key.1.clone(),
                    value: val.clone(),
                }),
                None => ops.push(WalOp::Delete {
                    store: key.0,
                    key: key.1.clone(),
                }),
            }
        }
        for (key, exists) in &self.pending_index_entries {
            if *exists {
                ops.push(WalOp::IndexPut {
                    index: key.0,
                    idx_key: key.1.clone(),
                    primary_key: key.2.clone(),
                });
            } else {
                ops.push(WalOp::IndexDelete {
                    index: key.0,
                    idx_key: key.1.clone(),
                    primary_key: key.2.clone(),
                });
            }
        }
        for (store, value) in &self.pending_key_generators {
            ops.push(WalOp::KeyGenSet {
                store: *store,
                value_bits: value.to_bits(),
            });
        }

        let mut meta = self.meta.clone();
        for (store, value) in &self.pending_key_generators {
            if let Some(s) = meta.stores.iter_mut().find(|s| s.id == *store) {
                s.key_gen = *value;
            }
        }
        let schema_dirty = self.schema_dirty || !self.pending_key_generators.is_empty();
        if schema_dirty {
            ops.push(WalOp::MetaReplace {
                bytes: encode_meta(&meta),
            });
        }
        (ops, meta, schema_dirty)
    }

    fn apply_pending_to_state(&self, meta: DatabaseMeta, txn_seq: u64, bump_seq: bool) {
        let mut state = self.state.write();
        if bump_seq {
            state.next_txn_seq = txn_seq.saturating_add(1);
        }
        for (key, value) in &self.pending_records {
            match value {
                Some(val) => {
                    state.records.insert_mut(key.clone(), val.clone());
                }
                None => {
                    state.records.remove_mut(key);
                }
            }
        }
        for (key, exists) in &self.pending_index_entries {
            if *exists {
                state.index_entries.insert_mut(key.clone(), ());
            } else {
                state.index_entries.remove_mut(key);
            }
        }
        for (store, value) in &self.pending_key_generators {
            state.key_generators.insert(*store, *value);
        }
        state.meta = Some(meta);
    }
}

impl BackendTxn for FsTxn {
    fn begin_request(&mut self) -> Result<(), BackendError> {
        self.undo_stack.push(Vec::new());
        self.key_delta_stack.push(self.key_delta);
        Ok(())
    }

    fn commit_request(&mut self) -> Result<(), BackendError> {
        let Some(ops) = self.undo_stack.pop() else {
            return Err(BackendError::Internal(
                "commit_request without matching begin_request".into(),
            ));
        };
        // Mutations persist into the parent scope, so the running delta is
        // already current: only the snapshot is discarded.
        let _ = self.key_delta_stack.pop();
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
        // The op replay below restores the frame-start map state exactly, so
        // restoring the frame-start delta snapshot restores the projection.
        if let Some(snapshot) = self.key_delta_stack.pop() {
            self.key_delta = snapshot;
        }
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
        self.schema_dirty = true;
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
        self.schema_dirty = true;
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
        self.schema_dirty = true;
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
        self.schema_dirty = true;
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
        self.schema_dirty = true;
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
        self.schema_dirty = true;
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
        self.schema_dirty = true;
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

        // Single existence probe serves the constraint, quota and delta
        // paths (previously probed twice per put).
        let existed = self.record_exists(&map_key);

        // Check no_overwrite
        if no_overwrite && existed {
            return Err(BackendError::Constraint(format!(
                "Record already exists for key in store {store}"
            )));
        }

        if !existed {
            let projected = self.projected_key_count_for_insert();
            if projected > self.max_keys_in_memory {
                return Err(BackendError::QuotaExceeded {
                    needed: projected,
                    available: self.max_keys_in_memory,
                });
            }
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
        Self::apply_record_transition(&mut self.key_delta, existed, true);
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
        Self::apply_record_transition(&mut self.key_delta, existed, false);
        Ok(existed)
    }

    fn delete_range(&mut self, store: StoreId, range: &EncodedRange) -> Result<u64, BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // Collect keys in range from committed storage. A set (not a Vec):
        // the pending-only sweep below probes membership per key, which was
        // quadratic in the range size (M7-B H-F1 hygiene).
        let state = self.state.read();
        let keys_in_range: std::collections::HashSet<RecordKey> = state
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
            let before = self.read_record(key).is_some();
            if before {
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
            Self::apply_record_transition(&mut self.key_delta, before, false);
        }

        // Delete pending-only records (not already handled).
        //
        // The previous pending value is logged for undo so a rollback
        // restores it; only records that actually exist are counted. Keys
        // here are absent from committed storage (committed in-range keys
        // are all in `keys_in_range`), so removal always ends absent.
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
                Self::apply_record_transition(&mut self.key_delta, true, false);
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
        let state = self.state.read();
        let mut keys: Vec<RecordKey> = state
            .records
            .keys()
            .filter(|(sid, _)| *sid == store)
            .cloned()
            .collect();
        for key in self.pending_records.keys().filter(|(sid, _)| *sid == store) {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }

        for key in keys {
            let old = self.pending_slot(&key);
            // Effective existence ahead of the tombstone: pending slots
            // decide locally, absent slots fall through to committed storage
            // (read under one guard for the whole sweep).
            let before = match &old {
                PendingSlot::Absent => state.records.contains_key(&key),
                PendingSlot::Deleted => false,
                PendingSlot::Value(_) => true,
            };
            if let Some(ops) = self.undo_stack.last_mut() {
                ops.push(UndoOp::RestoreRecord {
                    key: key.clone(),
                    old,
                });
            }
            // Tombstone (not removal): keeps the merged view consistent for
            // any further reads in this transaction.
            self.pending_records.insert(key, None);
            Self::apply_record_transition(&mut self.key_delta, before, false);
        }
        drop(state);

        Ok(())
    }

    fn count(&mut self, src: SourceRef, range: &EncodedRange) -> Result<u64, BackendError> {
        match src {
            SourceRef::Store(store) => {
                self.check_scope(store)?;
                let mut count: u64 = 0;

                // Count committed records in range
                let state = self.state.read();
                for (rkey, _) in state.records.iter() {
                    let (sid, key) = rkey;
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
                            let state = self.state.read();
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
                let state = self.state.read();
                for (ikey, _) in state.index_entries.iter() {
                    let (iid, idx_key, pk) = ikey;
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
        // Lazy cursors (M7-B H-MEM): snapshot the committed maps with
        // structural O(1) clones and merge them per step with the borrowed
        // pending overlay. No locks are held across steps.
        let maps = {
            let state = self.state.read();
            CursorMaps {
                records: state.records.clone(),
                index: state.index_entries.clone(),
                pending_records: &self.pending_records,
                pending_index: &self.pending_index_entries,
            }
        };
        match src {
            SourceRef::Store(store) => {
                self.check_scope(store)?;
                Ok(Box::new(FsCursor::open_store(
                    maps, store, range, dir, key_only,
                )))
            }
            SourceRef::Index { store, index } => {
                self.check_scope(store)?;
                Ok(Box::new(FsCursor::open_index(
                    maps, store, index, range, dir, key_only,
                )))
            }
        }
    }

    fn key_gen_current(&self, store: StoreId) -> Result<f64, BackendError> {
        // Check pending first
        if let Some(val) = self.pending_key_generators.get(&store) {
            return Ok(*val);
        }
        // Fall back to committed
        let state = self.state.read();
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
        // Readonly transactions never touch durable state.
        if self.mode == TxnMode::ReadOnly {
            return Ok(());
        }

        let sync = self.wants_sync();
        let (ops, meta, schema_dirty) = self.build_commit_payload();
        let empty = ops.is_empty();
        let wal_seq = self.state.read().wal_seq;
        let wal_file = wal_path(&self.db_dir, wal_seq);
        let wal_len_before = self.fs.metadata_len(&wal_file).unwrap_or(0);
        let txn_seq = self.state.read().next_txn_seq;

        let mut appended = 0u64;
        let mut frame_groups = 0u64;
        if !empty {
            let frames = match encode_txn_frames_limited(txn_seq, &ops, self.max_frame_payload) {
                Ok(frames) => frames,
                Err(CodecError::PayloadTooLarge(needed)) => {
                    return Err(BackendError::QuotaExceeded {
                        needed: needed as u64,
                        available: u64::from(self.max_frame_payload),
                    });
                }
                Err(err) => {
                    return Err(BackendError::Internal(format!("encode WAL frames: {err}")));
                }
            };
            frame_groups = 1;
            let wal_sync = sync || schema_dirty;
            for (i, bytes) in frames.iter().enumerate() {
                let is_last = i + 1 == frames.len();
                let do_sync = wal_sync && is_last;
                if let Err(err) = append_and_maybe_sync(&wal_file, bytes, do_sync, &self.fs) {
                    let _ = truncate_file(&wal_file, wal_len_before, &self.fs);
                    return Err(err);
                }
                appended += bytes.len() as u64;
            }
        }

        if schema_dirty {
            if let Err(err) = write_meta_file(&self.db_dir, &meta, sync, &self.fs) {
                let _ = truncate_file(&wal_file, wal_len_before, &self.fs);
                return Err(err);
            }
        }

        self.apply_pending_to_state(meta, txn_seq, !empty);
        if !empty {
            let mut state = self.state.write();
            state.wal_bytes_since_compact = state.wal_bytes_since_compact.saturating_add(appended);
            state.wal_frames_since_compact =
                state.wal_frames_since_compact.saturating_add(frame_groups);
            // Compaction errors must not undo a durable commit; retry next write.
            let _ = maybe_compact(&self.db_dir, &mut state, self.compact, &self.fs);
        }
        Ok(())
    }

    fn abort(self: Box<Self>) -> Result<(), BackendError> {
        Ok(())
    }
}
