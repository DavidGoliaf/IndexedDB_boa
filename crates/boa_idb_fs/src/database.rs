//! Open filesystem database with LOCK + WAL recovery.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use boa_idb_core::backend::capabilities::BackendCapabilities;
use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendTxn, Database};
use boa_idb_core::backend::types::DatabaseMeta;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, StoreId, TxnMode};
use parking_lot::RwLock;

use crate::apply::apply_frames;
use crate::atomic::{ensure_file, truncate_file};
use crate::lock::DbLock;
use crate::meta::{read_meta_file, write_manifest, write_meta_file};
use crate::state::DbState;
use crate::sync_hooks::{SyncHooks, io_to_backend};
use crate::txn::FsTxn;
use crate::wal::recover_committed_frames;

/// Open database handle holding the exclusive `LOCK`.
pub struct FsDatabase {
    meta: DatabaseMeta,
    state: Arc<RwLock<DbState>>,
    db_dir: PathBuf,
    hooks: Arc<dyn SyncHooks>,
    max_keys_in_memory: u64,
    max_frame_payload: u32,
    _lock: DbLock,
}

impl FsDatabase {
    /// Opens or creates a database directory under `db_dir`.
    pub fn open(
        db_dir: PathBuf,
        name: &str,
        hooks: Arc<dyn SyncHooks>,
        max_keys_in_memory: u64,
        max_frame_payload: u32,
    ) -> Result<Self, BackendError> {
        fs::create_dir_all(db_dir.join("wal")).map_err(|e| io_to_backend(e, "create wal dir"))?;
        let lock = DbLock::try_acquire(&db_dir.join("LOCK"))?;

        let current = db_dir.join("CURRENT");
        let wal_path = db_dir.join("wal").join("000001.log");
        if !current.exists() {
            // CURRENT is published last so a crash cannot leave CURRENT without meta.
            let meta = empty_meta(name);
            ensure_file(&wal_path)?;
            write_meta_file(&db_dir, &meta, true, &hooks)?;
            write_manifest(&db_dir, 1, b"boa_idb_fs m6a\n", true, &hooks)?;
        }

        let mut state = DbState {
            meta: read_meta_file(&db_dir)?,
            manifest_seq: 1,
            next_txn_seq: 1,
            ..DbState::default()
        };
        // Half-init heal: CURRENT/WAL without meta.scf (legacy crash window).
        if state.meta.is_none() {
            state.meta = Some(empty_meta(name));
        }
        if let Some(meta) = &state.meta {
            for store in &meta.stores {
                state.key_generators.insert(store.id, store.key_gen);
            }
        }

        ensure_file(&wal_path)?;
        let wal_bytes = fs::read(&wal_path).map_err(|e| io_to_backend(e, "read wal"))?;
        let recovered = recover_committed_frames(&wal_bytes);
        apply_frames(&mut state, &recovered.frames)?;
        if recovered.valid_prefix_len < wal_bytes.len() {
            truncate_file(&wal_path, recovered.valid_prefix_len as u64)?;
        }

        let meta = state
            .meta
            .clone()
            .ok_or_else(|| BackendError::Internal("database metadata missing after open".into()))?;

        // Keep meta.scf aligned with recovered state so list_databases matches open.
        let disk_meta = read_meta_file(&db_dir)?;
        if disk_meta.as_ref() != Some(&meta) {
            write_meta_file(&db_dir, &meta, true, &hooks)?;
        }

        Ok(Self {
            meta,
            state: Arc::new(RwLock::new(state)),
            db_dir,
            hooks,
            max_keys_in_memory,
            max_frame_payload,
            _lock: lock,
        })
    }
}

fn empty_meta(name: &str) -> DatabaseMeta {
    DatabaseMeta {
        name: Utf16String::from_str(name),
        version: 0,
        stores: Vec::new(),
        next_store_id: 1,
        next_index_id: 1,
    }
}

impl Database for FsDatabase {
    fn metadata(&self) -> &DatabaseMeta {
        &self.meta
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            snapshot_isolation: true,
            durable: true,
            concurrent: false,
            max_key_size: None,
            max_value_size: None,
        }
    }

    fn begin(
        &mut self,
        mode: TxnMode,
        scope: &[StoreId],
        durability: Durability,
    ) -> Result<Box<dyn BackendTxn + 'static>, BackendError> {
        if let Some(meta) = self.state.read().meta.clone() {
            self.meta = meta;
        }
        let state = if mode == TxnMode::ReadOnly {
            let snap = self.state.read().clone();
            Arc::new(RwLock::new(snap))
        } else {
            self.state.clone()
        };
        Ok(Box::new(FsTxn::new(
            mode,
            scope.to_vec(),
            self.meta.clone(),
            state,
            durability,
            self.db_dir.clone(),
            self.hooks.clone(),
            self.max_keys_in_memory,
            self.max_frame_payload,
        )))
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        let wal = self.db_dir.join("wal").join("000001.log");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&wal)
            .map_err(|e| io_to_backend(e, "flush open"))?;
        self.hooks
            .sync_file(&file)
            .map_err(|e| io_to_backend(e, "flush sync"))?;
        Ok(())
    }

    fn close(self: Box<Self>) -> Result<(), BackendError> {
        Ok(())
    }
}
