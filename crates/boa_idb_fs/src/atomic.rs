//! Atomic file replace: temp → write → sync → rename → dir sync.

use crate::vfs::{FileSystem, io_to_backend};
use boa_idb_core::backend::error::BackendError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Writes `data` to `target` atomically via a sibling temporary file.
pub fn atomic_write(
    target: &Path,
    data: &[u8],
    sync: bool,
    fs: &Arc<dyn FileSystem>,
) -> Result<(), BackendError> {
    let parent = target.parent().ok_or_else(|| {
        BackendError::Io(format!(
            "atomic write target has no parent: {}",
            target.display()
        ))
    })?;
    fs.create_dir_all(parent)
        .map_err(|e| io_to_backend(e, "create_dir_all"))?;

    let tmp = temp_path_for(target);
    fs.write_truncate(&tmp, data)
        .map_err(|e| io_to_backend(e, "write temp"))?;
    if sync {
        fs.sync_path(&tmp)
            .map_err(|e| io_to_backend(e, "sync temp"))?;
    }
    fs.rename(&tmp, target)
        .map_err(|e| io_to_backend(e, "rename"))?;
    if sync {
        fs.sync_dir(parent)
            .map_err(|e| io_to_backend(e, "sync dir"))?;
    }
    Ok(())
}

/// Appends bytes to a file, optionally syncing afterwards.
pub fn append_and_maybe_sync(
    path: &Path,
    data: &[u8],
    sync: bool,
    fs: &Arc<dyn FileSystem>,
) -> Result<(), BackendError> {
    fs.append(path, data)
        .map_err(|e| io_to_backend(e, "append wal"))?;
    if sync {
        fs.sync_path(path)
            .map_err(|e| io_to_backend(e, "sync wal"))?;
    }
    Ok(())
}

/// Truncates a file to `len` after successful validation.
pub fn truncate_file(path: &Path, len: u64, fs: &Arc<dyn FileSystem>) -> Result<(), BackendError> {
    fs.set_len(path, len)
        .map_err(|e| io_to_backend(e, "set_len"))
}

fn temp_path_for(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    target.with_file_name(format!(".{name}.tmp"))
}

/// Ensures a file exists (empty if newly created).
pub fn ensure_file(path: &Path, fs: &Arc<dyn FileSystem>) -> Result<(), BackendError> {
    fs.ensure_file(path)
        .map_err(|e| io_to_backend(e, "ensure file"))
}
