//! Injectable sync hooks for durability tests (R8.3.2 / R8.5.3 seam).
//!
//! Prefer [`crate::vfs::FileSystem`] for new code; hooks remain as a thin
//! adapter via [`crate::vfs::SyncHooksFs`].

use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// File/directory sync behaviour used by atomic writers and strict commit.
pub trait SyncHooks: Send + Sync + 'static {
    /// Synchronizes file contents (and metadata when the platform requires it).
    fn sync_file(&self, file: &File) -> io::Result<()>;

    /// Synchronizes a directory after a rename into it, when supported.
    fn sync_dir(&self, dir: &Path) -> io::Result<()>;
}

/// Production hooks: `File::sync_all` and best-effort directory sync.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsSyncHooks;

impl SyncHooks for OsSyncHooks {
    fn sync_file(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        // Opening a directory and syncing is platform-dependent; on Windows
        // `File::open` on a directory often fails. Best-effort: ignore
        // `IsADirectory`/`PermissionDenied` style failures after rename.
        match File::open(dir).and_then(|f| f.sync_all()) {
            Ok(()) => Ok(()),
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidInput
                ) =>
            {
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

/// Counting observer used by tests to distinguish strict vs relaxed commits.
#[derive(Debug, Default)]
pub struct CountingSyncHooks {
    /// Number of successful `sync_file` calls.
    pub file_syncs: AtomicU64,
    /// Number of successful `sync_dir` calls.
    pub dir_syncs: AtomicU64,
    inner: OsSyncHooks,
}

impl CountingSyncHooks {
    /// Creates a new counter wrapping OS sync.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Returns how many file syncs were observed.
    pub fn file_sync_count(&self) -> u64 {
        self.file_syncs.load(Ordering::SeqCst)
    }
}

impl SyncHooks for CountingSyncHooks {
    fn sync_file(&self, file: &File) -> io::Result<()> {
        self.inner.sync_file(file)?;
        self.file_syncs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        self.inner.sync_dir(dir)?;
        self.dir_syncs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
