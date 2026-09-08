//! Advisory exclusive lock on the database `LOCK` file (R8.3.6).

use crate::vfs::{FileSystem, FsLock};
use boa_idb_core::backend::error::BackendError;
use std::path::Path;
use std::sync::Arc;

/// Holds an exclusive advisory lock for the lifetime of this guard.
pub struct DbLock {
    _inner: FsLock,
}

impl DbLock {
    /// Tries to acquire an exclusive lock on `lock_path`.
    ///
    /// Returns [`BackendError::Locked`] when another process holds the lock.
    pub fn try_acquire(lock_path: &Path, fs: &Arc<dyn FileSystem>) -> Result<Self, BackendError> {
        Ok(Self {
            _inner: fs.try_lock_exclusive(lock_path)?,
        })
    }
}
