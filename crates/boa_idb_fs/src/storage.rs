//! Storage root listing databases under a storage-key directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{Database, Storage};

use crate::compact::CompactConfig;
use crate::database::FsDatabase;
use crate::lock::DbLock;
use crate::meta::load_meta_with_wal;
use crate::naming::database_dir_name;
use crate::state::SnapshotMeter;
use crate::vfs::{FileSystem, io_to_backend};

/// Filesystem storage for one storage key (origin).
pub struct FsStorage {
    root: PathBuf,
    storage_key: String,
    fs: Arc<dyn FileSystem>,
    max_keys_in_memory: u64,
    max_frame_payload: u32,
    compact: CompactConfig,
    meter: Arc<SnapshotMeter>,
}

impl FsStorage {
    pub(crate) fn new(
        root: PathBuf,
        storage_key: String,
        fs: Arc<dyn FileSystem>,
        max_keys_in_memory: u64,
        max_frame_payload: u32,
        compact: CompactConfig,
        meter: Arc<SnapshotMeter>,
    ) -> Result<Self, BackendError> {
        fs.create_dir_all(&root)
            .map_err(|e| io_to_backend(e, "create storage root"))?;
        Ok(Self {
            root,
            storage_key,
            fs,
            max_keys_in_memory,
            max_frame_payload,
            compact,
            meter,
        })
    }

    fn db_dir(&self, name: &str) -> PathBuf {
        self.root.join(database_dir_name(name))
    }
}

impl Storage for FsStorage {
    fn list_databases(&self) -> Result<Vec<(String, u64)>, BackendError> {
        let mut out = Vec::new();
        let entries = match self.fs.read_dir(&self.root) {
            Ok(e) => e,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(err) => return Err(io_to_backend(err, "list_databases")),
        };
        for entry in entries {
            if !entry.is_dir {
                continue;
            }
            if let Some(meta) = load_meta_with_wal(&entry.path, &self.fs)? {
                out.push((meta.name.to_string(), meta.version));
            }
        }
        Ok(out)
    }

    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError> {
        let db = FsDatabase::open(
            self.db_dir(name),
            name,
            self.fs.clone(),
            self.max_keys_in_memory,
            self.max_frame_payload,
            self.compact,
            self.meter.clone(),
        )?;
        Ok(Box::new(db))
    }

    fn delete_database(&self, name: &str) -> Result<(), BackendError> {
        let dir = self.db_dir(name);
        if self.fs.exists(&dir) {
            delete_database_dir(&dir, &self.fs)?;
        }
        let _ = self.storage_key;
        Ok(())
    }

    fn usage_bytes(&self) -> Result<u64, BackendError> {
        fn walk(fs: &dyn FileSystem, path: &Path) -> u64 {
            let mut total = 0u64;
            let Ok(entries) = fs.read_dir(path) else {
                return 0;
            };
            for entry in entries {
                if entry.is_dir {
                    total += walk(fs, &entry.path);
                } else if let Ok(len) = fs.metadata_len(&entry.path) {
                    total += len;
                }
            }
            total
        }
        Ok(walk(self.fs.as_ref(), &self.root))
    }
}

/// Deletes a database directory while holding `LOCK` across content removal.
///
/// On Windows an open `LOCK` handle prevents removing the directory itself, so
/// contents (except `LOCK`) are removed under the lock, then the lock is
/// dropped and the remaining files/dir are removed.
fn delete_database_dir(dir: &Path, fs: &Arc<dyn FileSystem>) -> Result<(), BackendError> {
    let lock = DbLock::try_acquire(&dir.join("LOCK"), fs)?;
    let entries = fs
        .read_dir(dir)
        .map_err(|e| io_to_backend(e, "delete read_dir"))?;
    for entry in entries {
        if entry.file_name == "LOCK" {
            continue;
        }
        if entry.is_dir {
            fs.remove_dir_all(&entry.path)
                .map_err(|e| io_to_backend(e, "delete nested dir"))?;
        } else {
            fs.remove_file(&entry.path)
                .map_err(|e| io_to_backend(e, "delete file"))?;
        }
    }
    drop(lock);
    let lock_path = dir.join("LOCK");
    if fs.exists(&lock_path) {
        fs.remove_file(&lock_path)
            .map_err(|e| io_to_backend(e, "delete LOCK"))?;
    }
    match fs.remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_to_backend(err, "delete_database")),
    }
}
