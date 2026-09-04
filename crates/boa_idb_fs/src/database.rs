//! Open filesystem database with LOCK + WAL recovery + segments.

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
use crate::compact::{CompactConfig, wal_path};
use crate::lock::DbLock;
use crate::meta::{ManifestData, load_manifest, read_meta_file, write_manifest, write_meta_file};
use crate::segment::load_segment_file;
use crate::state::{DbState, SnapshotMeter};
use crate::txn::FsTxn;
use crate::vfs::{FileSystem, io_to_backend};
use crate::wal::recover_committed_frames;

/// Open database handle holding the exclusive `LOCK`.
pub struct FsDatabase {
    meta: DatabaseMeta,
    state: Arc<RwLock<DbState>>,
    db_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    max_keys_in_memory: u64,
    max_frame_payload: u32,
    compact: CompactConfig,
    meter: Arc<SnapshotMeter>,
    _lock: DbLock,
}

impl FsDatabase {
    /// Opens or creates a database directory under `db_dir`.
    pub fn open(
        db_dir: PathBuf,
        name: &str,
        fs: Arc<dyn FileSystem>,
        max_keys_in_memory: u64,
        max_frame_payload: u32,
        compact: CompactConfig,
        meter: Arc<SnapshotMeter>,
    ) -> Result<Self, BackendError> {
        fs.create_dir_all(&db_dir.join("wal"))
            .map_err(|e| io_to_backend(e, "create wal dir"))?;
        fs.create_dir_all(&db_dir.join("seg"))
            .map_err(|e| io_to_backend(e, "create seg dir"))?;
        let lock = DbLock::try_acquire(&db_dir.join("LOCK"), &fs)?;

        let current = db_dir.join("CURRENT");
        if !fs.exists(&current) {
            let meta = empty_meta(name);
            let wal = wal_path(&db_dir, 1);
            ensure_file(&wal, &fs)?;
            write_meta_file(&db_dir, &meta, true, &fs)?;
            write_manifest(
                &db_dir,
                &ManifestData {
                    manifest_seq: 1,
                    wal_seq: 1,
                    segments: Vec::new(),
                },
                true,
                &fs,
            )?;
        }

        let mut state = DbState::default();
        let manifest = match load_manifest(&db_dir, &fs) {
            Ok(m) => m,
            Err(_) => {
                // Pre-B1 CURRENT bodies are not IMAN; treat as empty segment set.
                ManifestData {
                    manifest_seq: 1,
                    wal_seq: 1,
                    segments: Vec::new(),
                }
            }
        };
        state.manifest_seq = manifest.manifest_seq;
        state.wal_seq = manifest.wal_seq;

        for seq in &manifest.segments {
            let guard = load_segment_file(&db_dir, *seq, &mut state, &fs)?;
            state.segments.push(guard);
        }

        if state.meta.is_none() {
            state.meta = read_meta_file(&db_dir, &fs)?;
        }
        if state.meta.is_none() {
            state.meta = Some(empty_meta(name));
        }
        if let Some(meta) = &state.meta {
            for store in &meta.stores {
                state
                    .key_generators
                    .entry(store.id)
                    .or_insert(store.key_gen);
            }
        }

        let wal = wal_path(&db_dir, state.wal_seq);
        ensure_file(&wal, &fs)?;
        let wal_bytes = fs.read(&wal).map_err(|e| io_to_backend(e, "read wal"))?;
        let recovered = recover_committed_frames(&wal_bytes);
        apply_frames(&mut state, &recovered.frames)?;
        state.wal_bytes_since_compact = recovered.valid_prefix_len as u64;
        state.wal_frames_since_compact = recovered.frames.len() as u64;
        if recovered.valid_prefix_len < wal_bytes.len() {
            truncate_file(&wal, recovered.valid_prefix_len as u64, &fs)?;
        }

        let meta = state
            .meta
            .clone()
            .ok_or_else(|| BackendError::Internal("database metadata missing after open".into()))?;

        let disk_meta = read_meta_file(&db_dir, &fs)?;
        if disk_meta.as_ref() != Some(&meta) {
            write_meta_file(&db_dir, &meta, true, &fs)?;
        }

        Ok(Self {
            meta,
            state: Arc::new(RwLock::new(state)),
            db_dir,
            fs,
            max_keys_in_memory,
            max_frame_payload,
            compact,
            meter,
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
            let snap = self.state.read().snapshot_clone(&self.meter);
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
            self.fs.clone(),
            self.max_keys_in_memory,
            self.max_frame_payload,
            self.compact,
            self.meter.clone(),
        )))
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        let wal = wal_path(&self.db_dir, self.state.read().wal_seq);
        self.fs
            .sync_path(&wal)
            .map_err(|e| io_to_backend(e, "flush sync"))?;
        Ok(())
    }

    fn close(self: Box<Self>) -> Result<(), BackendError> {
        Ok(())
    }
}
