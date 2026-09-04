//! Atomic file replace: temp → write → sync → rename → dir sync.

use crate::sync_hooks::{SyncHooks, io_to_backend};
use boa_idb_core::backend::error::BackendError;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Writes `data` to `target` atomically via a sibling temporary file.
pub fn atomic_write(
    target: &Path,
    data: &[u8],
    sync: bool,
    hooks: &Arc<dyn SyncHooks>,
) -> Result<(), BackendError> {
    let parent = target.parent().ok_or_else(|| {
        BackendError::Io(format!(
            "atomic write target has no parent: {}",
            target.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|e| io_to_backend(e, "create_dir_all"))?;

    let tmp = temp_path_for(target);
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| io_to_backend(e, "open temp"))?;
        file.write_all(data)
            .map_err(|e| io_to_backend(e, "write temp"))?;
        if sync {
            hooks
                .sync_file(&file)
                .map_err(|e| io_to_backend(e, "sync temp"))?;
        }
    }
    fs::rename(&tmp, target).map_err(|e| io_to_backend(e, "rename"))?;
    if sync {
        hooks
            .sync_dir(parent)
            .map_err(|e| io_to_backend(e, "sync dir"))?;
    }
    Ok(())
}

/// Appends bytes to a file, optionally syncing afterwards.
pub fn append_and_maybe_sync(
    path: &Path,
    data: &[u8],
    sync: bool,
    hooks: &Arc<dyn SyncHooks>,
) -> Result<(), BackendError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| io_to_backend(e, "open wal append"))?;
    file.write_all(data)
        .map_err(|e| io_to_backend(e, "append wal"))?;
    if sync {
        hooks
            .sync_file(&file)
            .map_err(|e| io_to_backend(e, "sync wal"))?;
    }
    Ok(())
}

/// Truncates a file to `len` after successful validation.
pub fn truncate_file(path: &Path, len: u64) -> Result<(), BackendError> {
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| io_to_backend(e, "open truncate"))?;
    file.set_len(len).map_err(|e| io_to_backend(e, "set_len"))?;
    Ok(())
}

fn temp_path_for(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file");
    target.with_file_name(format!(".{name}.tmp"))
}

/// Ensures a file exists (empty if newly created).
pub fn ensure_file(path: &Path) -> Result<File, BackendError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_to_backend(e, "ensure parent"))?;
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| io_to_backend(e, "ensure file"))
}
