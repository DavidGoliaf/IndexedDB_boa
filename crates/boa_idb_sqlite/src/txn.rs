//! SQLite transaction with SAVEPOINT support (`r<seq>`).
//!
//! Transactions are managed via raw SQL (`BEGIN IMMEDIATE` / `BEGIN DEFERRED` /
//! `COMMIT` / `ROLLBACK`) rather than `rusqlite::Transaction` to avoid
//! lifetime complications with the connection pool guards.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendCursor, BackendTxn};
use boa_idb_core::backend::types::{DatabaseMeta, IndexSpec, StoreSpec};
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Direction, Durability, IndexId, SourceRef, StoreId, TxnMode};
use rusqlite::Connection;
use std::collections::HashMap;

use crate::blob::BlobManager;
use crate::cursor::SqliteCursor;
use crate::pool::Checkout;

/// SQLite transaction implementing `BackendTxn`.
///
/// Owns its checked-out connection for its whole lifetime (AD-6), so the
/// transaction is `'static` with no borrowed pool guards. Blob files are
/// tracked per savepoint level: files created by rolled-back work are
/// deleted when unreferenced, and files orphaned by overwrites/deletes are
/// collected at commit.
pub struct SqliteTxn {
    checkout: Checkout,
    mode: TxnMode,
    durability: Durability,
    scope: Vec<StoreId>,
    meta: DatabaseMeta,
    request_seq: u64,
    savepoints: Vec<String>,
    blob_manager: Option<BlobManager>,
    /// Blob rel-paths created per savepoint level (deleted on rollback/abort
    /// when nothing references them).
    blob_created: Vec<Vec<String>>,
    /// Blob rel-paths orphaned per savepoint level (deleted at commit when
    /// nothing references them).
    blob_orphaned: Vec<Vec<String>>,
    /// Index entries inserted via `index_put` since the last `put` overwrite of
    /// the same primary key. Used to keep `index_records` in sync when a record
    /// is overwritten (stale entries are removed, freshly inserted ones kept).
    pending_index_puts: HashMap<Vec<u8>, Vec<(IndexId, Vec<u8>)>>,
    committed: bool,
}

impl SqliteTxn {
    /// Creates a new writer transaction (ReadWrite or VersionChange).
    pub fn new_writer(
        checkout: Checkout,
        mode: TxnMode,
        durability: Durability,
        scope: Vec<StoreId>,
        meta: DatabaseMeta,
        blob_manager: Option<BlobManager>,
    ) -> Result<Self, BackendError> {
        {
            let conn = checkout.conn()?;
            if durability == Durability::Strict {
                conn.execute_batch("PRAGMA synchronous = FULL")
                    .map_err(|e| {
                        BackendError::Internal(format!("setting strict durability failed: {e}"))
                    })?;
            }
            conn.execute_batch("BEGIN IMMEDIATE").map_err(|e| {
                // Another connection may still hold the write lock (cross-pool
                // or multi-process). Surface as Locked so the L1 driver can
                // requeue instead of aborting the transaction.
                if matches!(
                    &e,
                    rusqlite::Error::SqliteFailure(err, _)
                        if err.code == rusqlite::ErrorCode::DatabaseBusy
                            || err.code == rusqlite::ErrorCode::DatabaseLocked
                ) {
                    BackendError::Locked
                } else {
                    BackendError::Internal(format!("BEGIN IMMEDIATE failed: {e}"))
                }
            })?;
        }

        Ok(Self {
            checkout,
            mode,
            durability,
            scope,
            meta,
            request_seq: 0,
            savepoints: Vec::new(),
            blob_manager,
            blob_created: Vec::new(),
            blob_orphaned: Vec::new(),
            pending_index_puts: HashMap::new(),
            committed: false,
        })
    }

    /// Creates a new reader transaction (ReadOnly).
    pub fn new_reader(
        checkout: Checkout,
        scope: Vec<StoreId>,
        meta: DatabaseMeta,
        blob_manager: Option<BlobManager>,
    ) -> Result<Self, BackendError> {
        {
            let conn = checkout.conn()?;
            conn.execute_batch("BEGIN DEFERRED")
                .map_err(|e| BackendError::Internal(format!("BEGIN DEFERRED failed: {e}")))?;
        }

        Ok(Self {
            checkout,
            mode: TxnMode::ReadOnly,
            durability: Durability::Default,
            scope,
            meta,
            request_seq: 0,
            savepoints: Vec::new(),
            blob_manager,
            blob_created: Vec::new(),
            blob_orphaned: Vec::new(),
            pending_index_puts: HashMap::new(),
            committed: false,
        })
    }

    /// Returns a reference to the underlying connection.
    fn conn(&self) -> Result<&Connection, BackendError> {
        self.checkout.conn()
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

    /// Validates that the store behind a scan/count source is in scope.
    fn check_src_scope(&self, src: SourceRef) -> Result<(), BackendError> {
        match src {
            SourceRef::Store(store) | SourceRef::Index { store, .. } => self.check_scope(store),
        }
    }

    /// Reads a record value, handling blob externalization.
    fn read_record_value(
        &self,
        store_id: StoreId,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, BackendError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare_cached("SELECT value, ext FROM records WHERE store_id = ?1 AND key = ?2")
            .map_err(|e| BackendError::Internal(format!("Failed to prepare get: {e}")))?;

        let result = stmt
            .query_row(rusqlite::params![store_id, key], |row| {
                let value: Option<Vec<u8>> = row.get(0)?;
                let ext: Option<String> = row.get(1)?;
                Ok((value, ext))
            })
            .optional()
            .map_err(|e| BackendError::Internal(format!("Get query failed: {e}")))?;

        match result {
            Some((value, ext)) => {
                if let Some(val) = value {
                    Ok(Some(val))
                } else if let Some(rel_path) = ext {
                    if let Some(bm) = self.blob_manager.as_ref() {
                        Ok(Some(bm.read(&rel_path)?))
                    } else {
                        Err(BackendError::Internal(
                            "Blob reference found but no blob manager".into(),
                        ))
                    }
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Ensures a blob-tracking level exists; returns its index.
    ///
    /// Operations may run outside an explicit savepoint (direct backend use
    /// in tests); the base level then collects their entries.
    fn blob_level_idx(&mut self) -> usize {
        if self.blob_created.is_empty() {
            debug_assert!(self.blob_orphaned.is_empty());
            self.blob_created.push(Vec::new());
            self.blob_orphaned.push(Vec::new());
        }
        self.blob_created.len() - 1
    }

    /// Records a newly externalized blob file in the current level.
    fn track_blob_created(&mut self, rel_path: String) {
        let idx = self.blob_level_idx();
        self.blob_created[idx].push(rel_path);
    }

    /// Records an orphaned blob reference in the current level.
    fn track_blob_orphaned(&mut self, rel_path: String) {
        let idx = self.blob_level_idx();
        self.blob_orphaned[idx].push(rel_path);
    }

    /// Returns true when `rel_path` is still referenced by a record.
    fn blob_is_referenced(&self, rel_path: &str) -> Result<bool, BackendError> {
        let count: i64 = self
            .conn()?
            .query_row(
                "SELECT COUNT(*) FROM records WHERE ext = ?1",
                [rel_path],
                |row| row.get(0),
            )
            .map_err(|e| BackendError::Internal(format!("blob refcheck failed: {e}")))?;
        Ok(count > 0)
    }

    /// Deletes a blob file unless still referenced (best-effort on IO errors:
    /// a leaked file is always safer than lost data).
    fn gc_blob_file(&self, rel_path: &str) -> Result<(), BackendError> {
        if self.blob_is_referenced(rel_path)? {
            return Ok(());
        }
        if let Some(bm) = self.blob_manager.as_ref() {
            let _ = bm.remove(rel_path);
        }
        Ok(())
    }

    /// Reads only the `ext` column of a record (cheap orphan check without
    /// resolving the value bytes).
    fn read_record_ext(
        &self,
        store_id: StoreId,
        key: &[u8],
    ) -> Result<Option<String>, BackendError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT ext FROM records WHERE store_id = ?1 AND key = ?2")
            .map_err(|e| BackendError::Internal(format!("Failed to prepare ext lookup: {e}")))?;
        let ext: Option<String> = stmt
            .query_row(rusqlite::params![store_id, key], |row| row.get(0))
            .optional()
            .map_err(|e| BackendError::Internal(format!("Ext lookup failed: {e}")))?
            .flatten();
        Ok(ext)
    }

    /// Collects `ext` references of records in a range (for orphan tracking).
    fn collect_range_exts(
        &self,
        store_id: StoreId,
        range: &EncodedRange,
    ) -> Result<Vec<String>, BackendError> {
        let (lower_sql, lower_params) = range_sql_lower(range);
        let (upper_sql, upper_params) = range_sql_upper(range);
        let sql = format!(
            "SELECT ext FROM records WHERE store_id = ?1 {lower_sql} {upper_sql} \
             AND ext IS NOT NULL"
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(store_id)];
        params.extend(lower_params);
        params.extend(upper_params);
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self
            .conn()?
            .prepare(&sql)
            .map_err(|e| BackendError::Internal(format!("Failed to prepare ext scan: {e}")))?;
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| row.get::<_, String>(0))
            .map_err(|e| BackendError::Internal(format!("Ext scan failed: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| BackendError::Internal(format!("Ext row error: {e}")))?);
        }
        Ok(out)
    }

    /// Collects `ext` references of all records of a store (for orphan
    /// tracking when the store itself is deleted).
    fn collect_store_exts(&self, store_id: StoreId) -> Result<Vec<String>, BackendError> {
        let mut stmt = self
            .conn()?
            .prepare("SELECT ext FROM records WHERE store_id = ?1 AND ext IS NOT NULL")
            .map_err(|e| BackendError::Internal(format!("Failed to prepare ext scan: {e}")))?;
        let rows = stmt
            .query_map([store_id], |row| row.get::<_, String>(0))
            .map_err(|e| BackendError::Internal(format!("Ext scan failed: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| BackendError::Internal(format!("Ext row error: {e}")))?);
        }
        Ok(out)
    }

    /// Removes stale index entries for a primary key before an overwrite.
    ///
    /// The engine inserts the new index entries (`index_put`) *before* the
    /// record upsert, so those entries are tracked in `pending_index_puts` and
    /// kept, while any other index entries for the same primary key are removed
    /// (the record's index key changed or disappeared).
    fn sync_index_entries_on_put(
        &mut self,
        store: StoreId,
        pkey: &[u8],
    ) -> Result<(), BackendError> {
        let keep = self.pending_index_puts.remove(pkey).unwrap_or_default();
        let index_ids: Vec<IndexId> = self
            .meta
            .stores
            .iter()
            .find(|s| s.id == store)
            .map(|s| s.indexes.iter().map(|i| i.id).collect())
            .unwrap_or_default();

        for index_id in index_ids {
            let keep_keys: Vec<&[u8]> = keep
                .iter()
                .filter(|(iid, _)| *iid == index_id)
                .map(|(_, k)| k.as_slice())
                .collect();

            if keep_keys.is_empty() {
                self.conn()?
                    .execute(
                        "DELETE FROM index_records WHERE index_id = ?1 AND pkey = ?2",
                        rusqlite::params![index_id, pkey],
                    )
                    .map_err(|e| BackendError::Internal(format!("index sync failed: {e}")))?;
            } else {
                let placeholders: Vec<String> = (0..keep_keys.len())
                    .map(|i| format!("?{}", i + 3))
                    .collect();
                let sql = format!(
                    "DELETE FROM index_records WHERE index_id = ?1 AND pkey = ?2 \
                     AND key NOT IN ({})",
                    placeholders.join(", ")
                );
                let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
                    vec![Box::new(index_id), Box::new(pkey.to_vec())];
                params.extend(
                    keep_keys
                        .into_iter()
                        .map(|k| Box::new(k.to_vec()) as Box<dyn rusqlite::types::ToSql>),
                );
                let refs: Vec<&dyn rusqlite::types::ToSql> =
                    params.iter().map(|p| p.as_ref()).collect();
                self.conn()?
                    .execute(&sql, refs.as_slice())
                    .map_err(|e| BackendError::Internal(format!("index sync failed: {e}")))?;
            }
        }

        Ok(())
    }

    /// Removes index entries belonging to the records of a store that are about
    /// to be deleted.
    fn delete_index_entries_for_store(&mut self, store: StoreId) -> Result<(), BackendError> {
        self.conn()?
            .execute(
                "DELETE FROM index_records \
                 WHERE index_id IN (SELECT id FROM indexes WHERE store_id = ?1)",
                [store],
            )
            .map_err(|e| BackendError::Internal(format!("index cleanup failed: {e}")))?;
        Ok(())
    }
}

/// Reports whether a rusqlite error is a uniqueness violation.
///
/// Only `PRIMARYKEY` (1555) and `UNIQUE` (2067) extended codes map to
/// `BackendError::Constraint`. Every other `SQLITE_CONSTRAINT` subcode
/// (`NOT NULL`, `FOREIGN KEY`, …) shares the primary `ConstraintViolation`
/// code but signals a real defect and must stay `Internal` with its message
/// intact — otherwise, e.g., a missing parent row would surface as a bogus
/// "already exists" error.
fn is_uniqueness_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ffi::ErrorCode::ConstraintViolation
                && (err.extended_code == 1555 || err.extended_code == 2067)
    )
}

impl BackendTxn for SqliteTxn {
    fn begin_request(&mut self) -> Result<(), BackendError> {
        let savepoint_name = format!("r{}", self.request_seq);
        self.request_seq += 1;
        self.conn()?
            .execute_batch(&format!("SAVEPOINT {savepoint_name}"))
            .map_err(|e| BackendError::Internal(format!("Savepoint begin failed: {e}")))?;
        self.savepoints.push(savepoint_name);
        self.blob_created.push(Vec::new());
        self.blob_orphaned.push(Vec::new());
        Ok(())
    }

    fn commit_request(&mut self) -> Result<(), BackendError> {
        let sp = self.savepoints.pop().ok_or_else(|| {
            BackendError::Internal("commit_request without matching begin_request".into())
        })?;
        self.conn()?
            .execute_batch(&format!("RELEASE SAVEPOINT {sp}"))
            .map_err(|e| BackendError::Internal(format!("Savepoint release failed: {e}")))?;
        // Merge blob tracking into the parent level (or re-base when none).
        let created = self.blob_created.pop().unwrap_or_default();
        let orphaned = self.blob_orphaned.pop().unwrap_or_default();
        let idx = self.blob_level_idx();
        self.blob_created[idx].extend(created);
        self.blob_orphaned[idx].extend(orphaned);
        Ok(())
    }

    fn rollback_request(&mut self) -> Result<(), BackendError> {
        let sp = self.savepoints.pop().ok_or_else(|| {
            BackendError::Internal("rollback_request without matching begin_request".into())
        })?;
        self.conn()?
            .execute_batch(&format!(
                "ROLLBACK TO SAVEPOINT {sp}; RELEASE SAVEPOINT {sp}"
            ))
            .map_err(|e| BackendError::Internal(format!("Savepoint rollback failed: {e}")))?;
        // The SQL rollback already restored row references: files created in
        // the rolled-back scope are garbage unless referenced, while orphaned
        // entries are live again and must be forgotten.
        let created = self.blob_created.pop().unwrap_or_default();
        self.blob_orphaned.pop();
        for rel_path in created {
            self.gc_blob_file(&rel_path)?;
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
        self.conn()?
            .execute(
                "INSERT OR REPLACE INTO meta (k, v) VALUES ('version', ?1)",
                [version.to_le_bytes().as_slice()],
            )
            .map_err(|e| BackendError::Internal(format!("set_version failed: {e}")))?;
        Ok(())
    }

    fn create_store(&mut self, spec: &StoreSpec) -> Result<StoreId, BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "create_store requires VersionChange mode".into(),
            ));
        }

        let name_bytes: Vec<u8> = spec
            .name
            .as_slice()
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let key_path_bytes = encode_key_path(&spec.key_path);

        self.conn()?
            .execute(
                "INSERT INTO object_stores (name, key_path, auto_increment, key_gen) \
                 VALUES (?1, ?2, ?3, 1.0)",
                rusqlite::params![
                    name_bytes,
                    key_path_bytes,
                    if spec.auto_increment { 1i32 } else { 0 },
                ],
            )
            .map_err(|e| {
                if is_uniqueness_violation(&e) {
                    BackendError::Constraint(format!("Object store '{}' already exists", spec.name))
                } else {
                    BackendError::Internal(format!("create_store failed: {e}"))
                }
            })?;

        let id = self.conn()?.last_insert_rowid() as StoreId;
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
        self.meta.next_store_id = self.meta.next_store_id.max(id + 1);
        Ok(id)
    }

    fn delete_store(&mut self, id: StoreId) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "delete_store requires VersionChange mode".into(),
            ));
        }
        // Collect external references before the cascade delete removes the
        // records: their blob files become orphans (collected at commit).
        for ext in self.collect_store_exts(id)? {
            self.track_blob_orphaned(ext);
        }
        self.conn()?
            .execute("DELETE FROM object_stores WHERE id = ?1", [id])
            .map_err(|e| BackendError::Internal(format!("delete_store failed: {e}")))?;
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
        let name_bytes: Vec<u8> = new_name
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        self.conn()?
            .execute(
                "UPDATE object_stores SET name = ?1 WHERE id = ?2",
                rusqlite::params![name_bytes, id],
            )
            .map_err(|e| {
                if is_uniqueness_violation(&e) {
                    BackendError::Constraint(format!("Object store '{new_name}' already exists"))
                } else {
                    BackendError::Internal(format!("rename_store failed: {e}"))
                }
            })?;
        if let Some(store) = self.meta.stores.iter_mut().find(|s| s.id == id) {
            store.name = Utf16String::from(new_name);
        }
        Ok(())
    }

    fn create_index(&mut self, store: StoreId, spec: &IndexSpec) -> Result<IndexId, BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "create_index requires VersionChange mode".into(),
            ));
        }

        let name_bytes: Vec<u8> = spec
            .name
            .as_slice()
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let key_path_bytes = encode_key_path(&spec.key_path);

        self.conn()?
            .execute(
                "INSERT INTO indexes (store_id, name, key_path, is_unique, multi_entry) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    store,
                    name_bytes,
                    // `None` (empty key path) would bind as NULL and violate
                    // `NOT NULL`; an empty blob decodes back to
                    // `KeyPath::Empty` all the same.
                    key_path_bytes.unwrap_or_default(),
                    if spec.unique { 1i32 } else { 0 },
                    if spec.multi_entry { 1i32 } else { 0 },
                ],
            )
            .map_err(|e| {
                if is_uniqueness_violation(&e) {
                    BackendError::Constraint(format!(
                        "Index '{}' already exists on store {store}",
                        spec.name
                    ))
                } else {
                    BackendError::Internal(format!("create_index failed: {e}"))
                }
            })?;

        let id = self.conn()?.last_insert_rowid() as IndexId;
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
        self.meta.next_index_id = self.meta.next_index_id.max(id + 1);
        Ok(id)
    }

    fn delete_index(&mut self, store: StoreId, id: IndexId) -> Result<(), BackendError> {
        if self.mode != TxnMode::VersionChange {
            return Err(BackendError::Internal(
                "delete_index requires VersionChange mode".into(),
            ));
        }
        self.conn()?
            .execute(
                "DELETE FROM indexes WHERE id = ?1 AND store_id = ?2",
                rusqlite::params![id, store],
            )
            .map_err(|e| BackendError::Internal(format!("delete_index failed: {e}")))?;
        if let Some(idx) = self
            .meta
            .stores
            .iter_mut()
            .find(|s| s.id == store)
            .and_then(|s| s.indexes.iter_mut().find(|i| i.id == id))
        {
            idx.deleted = true;
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
        let name_bytes: Vec<u8> = new_name
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        self.conn()?
            .execute(
                "UPDATE indexes SET name = ?1 WHERE id = ?2 AND store_id = ?3",
                rusqlite::params![name_bytes, id, store],
            )
            .map_err(|e| {
                if is_uniqueness_violation(&e) {
                    BackendError::Constraint(format!("Index '{new_name}' already exists"))
                } else {
                    BackendError::Internal(format!("rename_index failed: {e}"))
                }
            })?;
        if let Some(idx) = self
            .meta
            .stores
            .iter_mut()
            .find(|s| s.id == store)
            .and_then(|s| s.indexes.iter_mut().find(|i| i.id == id))
        {
            idx.name = Utf16String::from(new_name);
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

        let (value_col, ext_col, vlen): (Option<&[u8]>, Option<String>, i64) =
            if let Some(bm) = self.blob_manager.as_ref() {
                if BlobManager::should_externalize(value) {
                    let rel_path = bm.store(value)?;
                    self.track_blob_created(rel_path.clone());
                    (None, Some(rel_path), value.len() as i64)
                } else {
                    (Some(value), None, value.len() as i64)
                }
            } else {
                (Some(value), None, value.len() as i64)
            };

        // Track the previous external reference (if any) as orphaned: the
        // upsert below replaces it.
        if let Some(old_ext) = self.read_record_ext(store, key)? {
            // Same content re-put keeps its file: un-orphan on match is
            // handled by the reference check at collection time.
            self.track_blob_orphaned(old_ext);
        }

        // An upsert must keep the index consistent: drop stale index entries
        // for this primary key that are not re-inserted in this operation.
        if !no_overwrite {
            self.sync_index_entries_on_put(store, key)?;
        }

        let sql = if no_overwrite {
            "INSERT INTO records (store_id, key, value, ext, vlen) VALUES (?1, ?2, ?3, ?4, ?5)"
        } else {
            "INSERT INTO records (store_id, key, value, ext, vlen) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(store_id, key) DO UPDATE SET \
                 value = excluded.value, ext = excluded.ext, vlen = excluded.vlen"
        };

        self.conn()?
            .execute(sql, rusqlite::params![store, key, value_col, ext_col, vlen])
            .map_err(|e| {
                if is_uniqueness_violation(&e) {
                    BackendError::Constraint(format!(
                        "Record already exists for key in store {store}"
                    ))
                } else {
                    BackendError::Internal(format!("put failed: {e}"))
                }
            })?;

        Ok(())
    }

    fn get(&mut self, store: StoreId, key: &[u8]) -> Result<Option<Vec<u8>>, BackendError> {
        self.check_scope(store)?;
        self.read_record_value(store, key)
    }

    fn delete_record(&mut self, store: StoreId, key: &[u8]) -> Result<bool, BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // Track the orphaned blob reference (if any) before deleting.
        if let Some(old_ext) = self.read_record_ext(store, key)? {
            self.track_blob_orphaned(old_ext);
        }

        // Remove associated index entries before deleting the record.
        self.conn()?
            .execute(
                "DELETE FROM index_records \
                 WHERE index_id IN (SELECT id FROM indexes WHERE store_id = ?1) \
                   AND pkey = ?2",
                rusqlite::params![store, key],
            )
            .map_err(|e| BackendError::Internal(format!("index cleanup failed: {e}")))?;

        let changed = self
            .conn()?
            .execute(
                "DELETE FROM records WHERE store_id = ?1 AND key = ?2",
                rusqlite::params![store, key],
            )
            .map_err(|e| BackendError::Internal(format!("delete_record failed: {e}")))?;

        Ok(changed > 0)
    }

    fn delete_range(&mut self, store: StoreId, range: &EncodedRange) -> Result<u64, BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // Track orphaned blob references before deleting.
        for ext in self.collect_range_exts(store, range)? {
            self.track_blob_orphaned(ext);
        }

        // Remove index entries for the records being deleted first (the keys
        // must still be present in `records` for the subquery to match them).
        {
            let (lower_sql, lower_params) = range_sql_lower(range);
            let (upper_sql, upper_params) = range_sql_upper(range);

            let idx_sql = format!(
                "DELETE FROM index_records \
                 WHERE index_id IN (SELECT id FROM indexes WHERE store_id = ?1) \
                   AND pkey IN (SELECT key FROM records WHERE store_id = ?1 {lower_sql} {upper_sql})"
            );
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(store)];
            params.extend(lower_params);
            params.extend(upper_params);
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            self.conn()?
                .execute(&idx_sql, param_refs.as_slice())
                .map_err(|e| BackendError::Internal(format!("index cleanup failed: {e}")))?;
        }

        let (lower_sql, lower_params) = range_sql_lower(range);
        let (upper_sql, upper_params) = range_sql_upper(range);

        let sql = format!("DELETE FROM records WHERE store_id = ?1 {lower_sql} {upper_sql}");

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(store)];
        params.extend(lower_params);
        params.extend(upper_params);
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let changed = self
            .conn()?
            .execute(&sql, param_refs.as_slice())
            .map_err(|e| BackendError::Internal(format!("delete_range failed: {e}")))?;

        Ok(changed as u64)
    }

    fn clear(&mut self, store: StoreId) -> Result<(), BackendError> {
        self.check_scope(store)?;
        self.check_readwrite()?;

        // Track orphaned blob references before deleting.
        for ext in self.collect_range_exts(store, &EncodedRange::all())? {
            self.track_blob_orphaned(ext);
        }

        self.delete_index_entries_for_store(store)?;

        self.conn()?
            .execute("DELETE FROM records WHERE store_id = ?1", [store])
            .map_err(|e| BackendError::Internal(format!("clear failed: {e}")))?;

        Ok(())
    }

    fn count(&mut self, src: SourceRef, range: &EncodedRange) -> Result<u64, BackendError> {
        self.check_src_scope(src)?;

        let (sql, params) = crate::cursor::build_count_sql(src, range);
        let boxes: Vec<Box<dyn rusqlite::types::ToSql>> = params
            .iter()
            .map(crate::cursor::BoundParam::to_sql)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            boxes.iter().map(|b| b.as_ref()).collect();

        let count: i64 = self
            .conn()?
            .query_row(&sql, param_refs.as_slice(), |row| row.get(0))
            .map_err(|e| BackendError::Internal(format!("count failed: {e}")))?;
        Ok(count as u64)
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
                let cursor = SqliteCursor::open_store(
                    self.conn()?,
                    store,
                    range,
                    dir,
                    key_only,
                    self.blob_manager.as_ref(),
                );
                Ok(Box::new(cursor))
            }
            SourceRef::Index { store, index } => {
                self.check_scope(store)?;
                let cursor = SqliteCursor::open_index(
                    self.conn()?,
                    store,
                    index,
                    range,
                    dir,
                    key_only,
                    self.blob_manager.as_ref(),
                );
                Ok(Box::new(cursor))
            }
        }
    }

    fn key_gen_current(&self, store: StoreId) -> Result<f64, BackendError> {
        let key_gen_val: f64 = self
            .conn()?
            .query_row(
                "SELECT key_gen FROM object_stores WHERE id = ?1",
                [store],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    BackendError::NotFound(format!("Store {store} not found"))
                }
                _ => BackendError::Internal(format!("key_gen_current failed: {e}")),
            })?;
        Ok(key_gen_val)
    }

    fn key_gen_set(&mut self, store: StoreId, value: f64) -> Result<(), BackendError> {
        self.check_readwrite()?;
        self.conn()?
            .execute(
                "UPDATE object_stores SET key_gen = ?1 WHERE id = ?2",
                rusqlite::params![value, store],
            )
            .map_err(|e| BackendError::Internal(format!("key_gen_set failed: {e}")))?;
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

        if unique {
            let exists: bool = self
                .conn()?
                .query_row(
                    "SELECT COUNT(*) > 0 FROM index_records \
                     WHERE index_id = ?1 AND key = ?2 AND pkey != ?3",
                    rusqlite::params![index, idx_key, primary_key],
                    |row| row.get(0),
                )
                .map_err(|e| BackendError::Internal(format!("index_put check failed: {e}")))?;

            if exists {
                return Err(BackendError::Constraint(format!(
                    "Unique constraint violation for index {index}"
                )));
            }
        }

        self.conn()?
            .execute(
                "INSERT OR IGNORE INTO index_records (index_id, key, pkey) VALUES (?1, ?2, ?3)",
                rusqlite::params![index, idx_key, primary_key],
            )
            .map_err(|e| BackendError::Internal(format!("index_put failed: {e}")))?;

        // Track the entry so a subsequent record overwrite keeps it instead of
        // treating it as stale.
        self.pending_index_puts
            .entry(primary_key.to_vec())
            .or_default()
            .push((index, idx_key.to_vec()));

        Ok(())
    }

    fn index_delete(
        &mut self,
        index: IndexId,
        idx_key: &[u8],
        primary_key: &[u8],
    ) -> Result<(), BackendError> {
        self.check_readwrite()?;
        self.conn()?
            .execute(
                "DELETE FROM index_records \
                 WHERE index_id = ?1 AND key = ?2 AND pkey = ?3",
                rusqlite::params![index, idx_key, primary_key],
            )
            .map_err(|e| BackendError::Internal(format!("index_delete failed: {e}")))?;
        Ok(())
    }

    fn index_delete_by_primary(
        &mut self,
        index: IndexId,
        primary_key: &[u8],
    ) -> Result<(), BackendError> {
        self.check_readwrite()?;
        self.conn()?
            .execute(
                "DELETE FROM index_records WHERE index_id = ?1 AND pkey = ?2",
                rusqlite::params![index, primary_key],
            )
            .map_err(|e| BackendError::Internal(format!("index_delete_by_primary failed: {e}")))?;
        Ok(())
    }

    fn commit(mut self: Box<Self>) -> Result<(), BackendError> {
        // Persist the database version only for VersionChange transactions.
        // `set_version` has already written the value inside the transaction,
        // so re-writing it here with a possibly-stale snapshot would clobber it.
        if self.mode == TxnMode::VersionChange {
            self.conn()?
                .execute(
                    "INSERT OR REPLACE INTO meta (k, v) VALUES ('version', ?1)",
                    [self.meta.version.to_le_bytes().as_slice()],
                )
                .map_err(|e| BackendError::Internal(format!("commit meta write failed: {e}")))?;
        }

        self.conn()?
            .execute_batch("COMMIT")
            .map_err(|e| BackendError::Internal(format!("COMMIT failed: {e}")))?;

        if self.durability == Durability::Strict {
            self.conn()?
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .map_err(|e| {
                    BackendError::Internal(format!("strict WAL checkpoint failed: {e}"))
                })?;
            // Connections are pooled. Do not leak FULL into a later default
            // transaction after the strict commit has been made durable.
            self.conn()?
                .execute_batch("PRAGMA synchronous = NORMAL")
                .map_err(|e| {
                    BackendError::Internal(format!("restoring SQLite durability failed: {e}"))
                })?;
        }

        // Collect orphaned blob files: references are final now, so whatever
        // is still unreferenced can go.
        let orphaned: Vec<String> = self.blob_orphaned.drain(..).flatten().collect();
        self.blob_created.clear();
        for rel_path in orphaned {
            self.gc_blob_file(&rel_path)?;
        }

        self.committed = true;
        Ok(())
    }

    fn abort(mut self: Box<Self>) -> Result<(), BackendError> {
        // Rollback any remaining savepoints (best-effort).
        while let Some(sp) = self.savepoints.pop() {
            if let Ok(conn) = self.conn() {
                let _ = conn.execute_batch(&format!(
                    "ROLLBACK TO SAVEPOINT {sp}; RELEASE SAVEPOINT {sp}"
                ));
            }
        }

        self.conn()?
            .execute_batch("ROLLBACK")
            .map_err(|e| BackendError::Internal(format!("ROLLBACK failed: {e}")))?;

        // Delete files created by this transaction when unreferenced; the
        // orphaned entries are live again after the rollback.
        let created: Vec<String> = std::mem::take(&mut self.blob_created)
            .into_iter()
            .flatten()
            .collect();
        self.blob_orphaned.clear();
        for rel_path in created {
            self.gc_blob_file(&rel_path)?;
        }

        self.committed = true;
        Ok(())
    }
}

impl Drop for SqliteTxn {
    fn drop(&mut self) {
        if !self.committed {
            // Best-effort rollback; the connection is always available here
            // because the checkout is only released after this.
            let savepoints = std::mem::take(&mut self.savepoints);
            if let Ok(conn) = self.conn() {
                for sp in savepoints {
                    let _ = conn.execute_batch(&format!(
                        "ROLLBACK TO SAVEPOINT {sp}; RELEASE SAVEPOINT {sp}"
                    ));
                }
                let _ = conn.execute_batch("ROLLBACK");
            }
            // Best-effort file cleanup for created-but-never-committed blobs.
            let created: Vec<String> = std::mem::take(&mut self.blob_created)
                .into_iter()
                .flatten()
                .collect();
            for rel_path in created {
                let _ = self.gc_blob_file(&rel_path);
            }
        }
    }
}

/// Encodes a `KeyPath` into its on-disk BLOB form.
///
/// Format: a single tag byte followed by the payload.
/// - `0x01` single key path: UTF-16LE bytes of the path string.
/// - `0x02` array key path: sequence of `u32` (LE) length + UTF-16LE bytes.
///
/// `KeyPath::Empty` is stored as `NULL` (encoded as `None`).
fn encode_key_path(kp: &boa_idb_core::key::path::KeyPath) -> Option<Vec<u8>> {
    use boa_idb_core::key::path::KeyPath;
    match kp {
        KeyPath::Empty => None,
        KeyPath::Single(s) => {
            let mut bytes = vec![0x01];
            bytes.extend(s.as_slice().iter().flat_map(|u| u.to_le_bytes()));
            Some(bytes)
        }
        KeyPath::Array(paths) => {
            let mut bytes = vec![0x02];
            for p in paths {
                let pbytes: Vec<u8> = p.as_slice().iter().flat_map(|u| u.to_le_bytes()).collect();
                bytes.extend_from_slice(&(pbytes.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&pbytes);
            }
            Some(bytes)
        }
    }
}

/// Builds SQL fragment and params for a range lower bound.
fn range_sql_lower(range: &EncodedRange) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    match &range.lower {
        Some((key, open)) => {
            let op = if *open { ">" } else { ">=" };
            (format!("AND key {op} ?"), vec![Box::new(key.clone())])
        }
        None => (String::new(), Vec::new()),
    }
}

/// Builds SQL fragment and params for a range upper bound.
fn range_sql_upper(range: &EncodedRange) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    match &range.upper {
        Some((key, open)) => {
            let op = if *open { "<" } else { "<=" };
            (format!("AND key {op} ?"), vec![Box::new(key.clone())])
        }
        None => (String::new(), Vec::new()),
    }
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::ConnectionPool;
    use boa_idb_core::backend::types::DatabaseMeta;

    #[test]
    fn strict_commit_uses_full_sync_and_restores_pool_connection() {
        let temp = tempfile::tempdir().unwrap();
        let pool = ConnectionPool::open(&temp.path().join("strict.db")).unwrap();
        let checkout = pool.checkout_writer().unwrap();
        let txn = SqliteTxn::new_writer(
            checkout,
            TxnMode::ReadWrite,
            Durability::Strict,
            Vec::new(),
            DatabaseMeta {
                name: Utf16String::default(),
                version: 0,
                stores: Vec::new(),
                next_store_id: 1,
                next_index_id: 1,
            },
            None,
        )
        .unwrap();

        let synchronous: i64 = txn
            .conn()
            .unwrap()
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        assert_eq!(synchronous, 2, "strict transactions must use FULL sync");

        Box::new(txn).commit().unwrap();
        let checkout = pool.checkout_writer().unwrap();
        let synchronous: i64 = checkout
            .conn()
            .unwrap()
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            synchronous, 1,
            "pooled connections must restore NORMAL sync"
        );
    }
}
