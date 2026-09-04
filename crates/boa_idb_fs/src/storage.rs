//! Storage root listing databases under a storage-key directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{Database, Storage};

use crate::database::FsDatabase;
use crate::lock::DbLock;
use crate::meta::load_meta_with_wal;
use crate::naming::database_dir_name;
use crate::sync_hooks::{SyncHooks, io_to_backend};

/// Filesystem storage for one storage key (origin).
pub struct FsStorage {
    root: PathBuf,
    storage_key: String,
    hooks: Arc<dyn SyncHooks>,
    max_keys_in_memory: u64,
    max_frame_payload: u32,
}

impl FsStorage {
    pub(crate) fn new(
        root: PathBuf,
        storage_key: String,
        hooks: Arc<dyn SyncHooks>,
        max_keys_in_memory: u64,
        max_frame_payload: u32,
    ) -> Result<Self, BackendError> {
        fs::create_dir_all(&root).map_err(|e| io_to_backend(e, "create storage root"))?;
        Ok(Self {
            root,
            storage_key,
            hooks,
            max_keys_in_memory,
            max_frame_payload,
        })
    }

    fn db_dir(&self, name: &str) -> PathBuf {
        self.root.join(database_dir_name(name))
    }
}

impl Storage for FsStorage {
    fn list_databases(&self) -> Result<Vec<(String, u64)>, BackendError> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(err) => return Err(io_to_backend(err, "list_databases")),
        };
        for entry in entries {
            let entry = entry.map_err(|e| io_to_backend(e, "list entry"))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(meta) = load_meta_with_wal(&path)? {
                out.push((meta.name.to_string(), meta.version));
            }
        }
        Ok(out)
    }

    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError> {
        let db = FsDatabase::open(
            self.db_dir(name),
            name,
            self.hooks.clone(),
            self.max_keys_in_memory,
            self.max_frame_payload,
        )?;
        Ok(Box::new(db))
    }

    fn delete_database(&self, name: &str) -> Result<(), BackendError> {
        let dir = self.db_dir(name);
        if dir.exists() {
            delete_database_dir(&dir)?;
        }
        let _ = self.storage_key;
        Ok(())
    }

    fn usage_bytes(&self) -> Result<u64, BackendError> {
        fn walk(path: &Path) -> u64 {
            let mut total = 0u64;
            let Ok(entries) = fs::read_dir(path) else {
                return 0;
            };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    total += walk(&p);
                } else if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
            total
        }
        Ok(walk(&self.root))
    }
}

/// Deletes a database directory while holding `LOCK` across content removal.
///
/// On Windows an open `LOCK` handle prevents removing the directory itself, so
/// contents (except `LOCK`) are removed under the lock, then the lock is
/// dropped and the remaining files/dir are removed.
fn delete_database_dir(dir: &Path) -> Result<(), BackendError> {
    let lock = DbLock::try_acquire(&dir.join("LOCK"))?;
    let entries = fs::read_dir(dir).map_err(|e| io_to_backend(e, "delete read_dir"))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_to_backend(e, "delete entry"))?;
        if entry.file_name() == *"LOCK" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(|e| io_to_backend(e, "delete nested dir"))?;
        } else {
            fs::remove_file(&path).map_err(|e| io_to_backend(e, "delete file"))?;
        }
    }
    drop(lock);
    let lock_path = dir.join("LOCK");
    if lock_path.exists() {
        fs::remove_file(&lock_path).map_err(|e| io_to_backend(e, "delete LOCK"))?;
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_to_backend(err, "delete_database")),
    }
}
