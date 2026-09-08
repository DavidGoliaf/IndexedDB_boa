//! Advisory exclusive lock on the database `LOCK` file (R8.3.6).

use crate::sync_hooks::io_to_backend;
use boa_idb_core::backend::error::BackendError;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// Holds an exclusive advisory lock for the lifetime of this guard.
pub struct DbLock {
    file: File,
}

impl DbLock {
    /// Tries to acquire an exclusive lock on `lock_path`.
    ///
    /// Returns [`BackendError::Locked`] when another process holds the lock.
    pub fn try_acquire(lock_path: &Path) -> Result<Self, BackendError> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_to_backend(e, "lock parent"))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| io_to_backend(e, "open LOCK"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(BackendError::Locked),
            Err(TryLockError::Error(err)) => Err(io_to_backend(err, "try_lock")),
        }
    }
}

impl Drop for DbLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
