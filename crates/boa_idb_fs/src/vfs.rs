//! Virtual filesystem seam for production IO and fault injection (M6-B2 / R8.5.2).

use boa_idb_core::backend::error::BackendError;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::sync_hooks::SyncHooks;

/// Maps IO errors into backend errors without panicking.
pub fn io_to_backend(err: io::Error, context: &str) -> BackendError {
    if err.kind() == io::ErrorKind::StorageFull {
        return BackendError::QuotaExceeded {
            needed: 0,
            available: 0,
        };
    }
    BackendError::Io(format!("{context}: {err}"))
}

/// Publication / IO stage where a deterministic fault may fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaultSite {
    /// Append to an active WAL file.
    WalAppend,
    /// Sync of a WAL file.
    WalSync,
    /// Segment body write / rename into place.
    SegmentWrite,
    /// Manifest file write / rename.
    ManifestWrite,
    /// `CURRENT` body write (temp file before rename).
    CurrentWrite,
    /// `CURRENT` rename (publication tip).
    CurrentRename,
    /// `meta.scf` write.
    MetaWrite,
    /// Directory sync after rename.
    DirSync,
    /// Cleanup / unlink of superseded files.
    CleanupRemove,
    /// Exclusive `LOCK` acquire.
    LockAcquire,
    /// Truncate (WAL recovery / rotation).
    Truncate,
}

/// Deterministic fault kinds required by R8.5.2 / R8.5.3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultKind {
    /// Disk full (`ENOSPC` / `StorageFull`).
    Enospc,
    /// Generic I/O failure.
    Eio,
    /// Persist only the first `bytes`, then return an I/O error.
    ///
    /// The partial bytes are left on disk (torn physical write), but the call
    /// fails so the atomic replacement path (temp → sync → rename) never
    /// publishes a truncated tip. This matches `write_all`-style failure,
    /// not a silent Ok.
    ShortWrite {
        /// Bytes actually written before the error.
        bytes: usize,
    },
    /// Fail during sync.
    SyncFail,
    /// Fail during rename.
    RenameFail,
    /// Interrupted / aborted operation.
    Interrupt,
}

fn fault_io(kind: &FaultKind) -> io::Error {
    match kind {
        FaultKind::Enospc => io::Error::new(io::ErrorKind::StorageFull, "fault: ENOSPC"),
        FaultKind::Eio => io::Error::other("fault: EIO"),
        FaultKind::SyncFail => io::Error::other("fault: sync failed"),
        FaultKind::RenameFail => io::Error::other("fault: rename failed"),
        FaultKind::Interrupt => io::Error::new(io::ErrorKind::Interrupted, "fault: interrupt"),
        FaultKind::ShortWrite { bytes } => io::Error::new(
            io::ErrorKind::WriteZero,
            format!("fault: short write after {bytes} bytes"),
        ),
    }
}

/// Writes a prefix then returns a short-write error (never Ok).
fn short_write_then_err(
    write: impl FnOnce(&[u8]) -> io::Result<()>,
    data: &[u8],
    bytes: usize,
) -> io::Result<()> {
    let n = bytes.min(data.len());
    write(&data[..n])?;
    Err(fault_io(&FaultKind::ShortWrite { bytes: n }))
}

/// Directory entry returned by [`FileSystem::read_dir`].
#[derive(Debug, Clone)]
pub struct FsDirEntry {
    /// Absolute (or joined) path of the entry.
    pub path: PathBuf,
    /// File name component.
    pub file_name: String,
    /// Whether the entry is a directory.
    pub is_dir: bool,
}

/// Exclusive advisory lock guard released on drop.
pub struct FsLock {
    file: File,
}

impl Drop for FsLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Internal filesystem used by all production FS-backend IO.
pub trait FileSystem: Send + Sync + 'static {
    /// Creates a directory and all parents.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    /// Reads an entire file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Reads an entire file as UTF-8 text.
    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    /// Creates/truncates `path` and writes `data`.
    fn write_truncate(&self, path: &Path, data: &[u8]) -> io::Result<()>;

    /// Appends `data` to `path`, creating the file if needed.
    fn append(&self, path: &Path, data: &[u8]) -> io::Result<()>;

    /// Sets file length.
    fn set_len(&self, path: &Path, len: u64) -> io::Result<()>;

    /// Renames `from` to `to`.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Removes a single file.
    fn remove_file(&self, path: &Path) -> io::Result<()>;

    /// Removes a directory tree.
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;

    /// Returns file length.
    fn metadata_len(&self, path: &Path) -> io::Result<u64>;

    /// Returns whether `path` exists.
    fn exists(&self, path: &Path) -> bool;

    /// Lists directory entries.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<FsDirEntry>>;

    /// Opens `path` and syncs file contents.
    fn sync_path(&self, path: &Path) -> io::Result<()>;

    /// Best-effort directory sync after rename.
    fn sync_dir(&self, path: &Path) -> io::Result<()>;

    /// Ensures a file exists (empty if newly created).
    fn ensure_file(&self, path: &Path) -> io::Result<()>;

    /// Tries to acquire an exclusive advisory lock on `path`.
    fn try_lock_exclusive(&self, path: &Path) -> Result<FsLock, BackendError>;
}

/// Production OS filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsFileSystem;

impl OsFileSystem {
    /// Shared instance for factories.
    pub fn shared() -> Arc<dyn FileSystem> {
        Arc::new(Self)
    }
}

impl FileSystem for OsFileSystem {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    fn write_truncate(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        file.write_all(data)?;
        Ok(())
    }

    fn append(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        file.write_all(data)?;
        Ok(())
    }

    fn set_len(&self, path: &Path, len: u64) -> io::Result<()> {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(len)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        fs::remove_dir_all(path)
    }

    fn metadata_len(&self, path: &Path) -> io::Result<u64> {
        Ok(fs::metadata(path)?.len())
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<FsDirEntry>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type()?.is_dir();
            out.push(FsDirEntry {
                path: entry.path(),
                file_name,
                is_dir,
            });
        }
        Ok(out)
    }

    fn sync_path(&self, path: &Path) -> io::Result<()> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        file.sync_all()
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        match File::open(path).and_then(|f| f.sync_all()) {
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

    fn ensure_file(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _ = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        Ok(())
    }

    fn try_lock_exclusive(&self, path: &Path) -> Result<FsLock, BackendError> {
        if let Some(parent) = path.parent() {
            self.create_dir_all(parent)
                .map_err(|e| io_to_backend(e, "lock parent"))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| io_to_backend(e, "open LOCK"))?;
        match file.try_lock() {
            Ok(()) => Ok(FsLock { file }),
            Err(TryLockError::WouldBlock) => Err(BackendError::Locked),
            Err(TryLockError::Error(err)) => Err(io_to_backend(err, "try_lock")),
        }
    }
}

/// Adapts legacy [`SyncHooks`] into [`FileSystem`] for existing tests.
pub struct SyncHooksFs {
    hooks: Arc<dyn SyncHooks>,
    inner: OsFileSystem,
    /// Optional counter for file syncs (mirrors CountingSyncHooks).
    pub file_syncs: AtomicU64,
    /// Optional counter for dir syncs.
    pub dir_syncs: AtomicU64,
}

impl SyncHooksFs {
    /// Wraps OS IO with injectable sync hooks.
    pub fn new(hooks: Arc<dyn SyncHooks>) -> Arc<Self> {
        Arc::new(Self {
            hooks,
            inner: OsFileSystem,
            file_syncs: AtomicU64::new(0),
            dir_syncs: AtomicU64::new(0),
        })
    }
}

impl FileSystem for SyncHooksFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(path)
    }

    fn write_truncate(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        self.inner.write_truncate(path, data)
    }

    fn append(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        self.inner.append(path, data)
    }

    fn set_len(&self, path: &Path, len: u64) -> io::Result<()> {
        self.inner.set_len(path, len)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_dir_all(path)
    }

    fn metadata_len(&self, path: &Path) -> io::Result<u64> {
        self.inner.metadata_len(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<FsDirEntry>> {
        self.inner.read_dir(path)
    }

    fn sync_path(&self, path: &Path) -> io::Result<()> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        self.hooks.sync_file(&file)?;
        self.file_syncs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        self.hooks.sync_dir(path)?;
        self.dir_syncs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn ensure_file(&self, path: &Path) -> io::Result<()> {
        self.inner.ensure_file(path)
    }

    fn try_lock_exclusive(&self, path: &Path) -> Result<FsLock, BackendError> {
        self.inner.try_lock_exclusive(path)
    }
}

#[derive(Debug, Clone)]
struct FaultRule {
    site: FaultSite,
    kind: FaultKind,
    remaining: u32,
}

/// Test filesystem that injects deterministic faults without changing control flow.
pub struct FaultInjectingFs {
    inner: Arc<dyn FileSystem>,
    rules: Mutex<Vec<FaultRule>>,
}

impl FaultInjectingFs {
    /// Wraps `inner` with an empty fault plan.
    pub fn new(inner: Arc<dyn FileSystem>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            rules: Mutex::new(Vec::new()),
        })
    }

    /// Queues a one-shot fault at `site`.
    pub fn inject_once(&self, site: FaultSite, kind: FaultKind) {
        self.inject_n(site, kind, 1);
    }

    /// Queues a fault that fires `n` times at `site`.
    pub fn inject_n(&self, site: FaultSite, kind: FaultKind, n: u32) {
        if n == 0 {
            return;
        }
        self.rules.lock().expect("fault rules").push(FaultRule {
            site,
            kind,
            remaining: n,
        });
    }

    /// Clears all pending faults.
    pub fn clear(&self) {
        self.rules.lock().expect("fault rules").clear();
    }

    fn take_fault(&self, site: FaultSite) -> Option<FaultKind> {
        let mut rules = self.rules.lock().expect("fault rules");
        let idx = rules
            .iter()
            .position(|r| r.site == site && r.remaining > 0)?;
        let rule = &mut rules[idx];
        rule.remaining = rule.remaining.saturating_sub(1);
        let kind = rule.kind.clone();
        if rule.remaining == 0 {
            rules.remove(idx);
        }
        Some(kind)
    }
}

fn bare_name(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if let Some(stripped) = name.strip_prefix('.') {
        if let Some(inner) = stripped.strip_suffix(".tmp") {
            return inner.to_string();
        }
    }
    name
}

/// Classifies a write/truncate target (not the `CURRENT` rename tip).
fn classify_write(path: &Path) -> FaultSite {
    let name = bare_name(path);
    let lossy = path.to_string_lossy();
    // CURRENT body is written to a temp file; rename is [`FaultSite::CurrentRename`].
    if name == "CURRENT" {
        return FaultSite::CurrentWrite;
    }
    if name.starts_with("MANIFEST") {
        return FaultSite::ManifestWrite;
    }
    if name == "meta.scf" || name.starts_with("meta.scf") {
        return FaultSite::MetaWrite;
    }
    if Path::new(&name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("seg"))
    {
        return FaultSite::SegmentWrite;
    }
    if Path::new(&name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
        || lossy.contains("wal")
    {
        return FaultSite::WalAppend;
    }
    FaultSite::MetaWrite
}

fn classify_sync(path: &Path) -> FaultSite {
    match classify_write(path) {
        FaultSite::WalAppend => FaultSite::WalSync,
        other => other,
    }
}

fn classify_rename_target(to: &Path) -> FaultSite {
    if bare_name(to) == "CURRENT" {
        FaultSite::CurrentRename
    } else {
        classify_write(to)
    }
}

impl FileSystem for FaultInjectingFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(path)
    }

    fn write_truncate(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let site = classify_write(path);
        if let Some(kind) = self.take_fault(site) {
            match kind {
                FaultKind::ShortWrite { bytes } => {
                    return short_write_then_err(
                        |prefix| self.inner.write_truncate(path, prefix),
                        data,
                        bytes,
                    );
                }
                other => return Err(fault_io(&other)),
            }
        }
        self.inner.write_truncate(path, data)
    }

    fn append(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let site = FaultSite::WalAppend;
        if let Some(kind) = self.take_fault(site) {
            match kind {
                FaultKind::ShortWrite { bytes } => {
                    return short_write_then_err(
                        |prefix| self.inner.append(path, prefix),
                        data,
                        bytes,
                    );
                }
                other => return Err(fault_io(&other)),
            }
        }
        self.inner.append(path, data)
    }

    fn set_len(&self, path: &Path, len: u64) -> io::Result<()> {
        if let Some(kind) = self.take_fault(FaultSite::Truncate) {
            return Err(fault_io(&kind));
        }
        self.inner.set_len(path, len)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let site = classify_rename_target(to);
        if let Some(kind) = self.take_fault(site) {
            match kind {
                FaultKind::RenameFail
                | FaultKind::Enospc
                | FaultKind::Eio
                | FaultKind::Interrupt
                | FaultKind::SyncFail => {
                    return Err(fault_io(&FaultKind::RenameFail));
                }
                FaultKind::ShortWrite { .. } => {}
            }
        }
        self.inner.rename(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        if let Some(kind) = self.take_fault(FaultSite::CleanupRemove) {
            return Err(fault_io(&kind));
        }
        self.inner.remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        if let Some(kind) = self.take_fault(FaultSite::CleanupRemove) {
            return Err(fault_io(&kind));
        }
        self.inner.remove_dir_all(path)
    }

    fn metadata_len(&self, path: &Path) -> io::Result<u64> {
        self.inner.metadata_len(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<FsDirEntry>> {
        self.inner.read_dir(path)
    }

    fn sync_path(&self, path: &Path) -> io::Result<()> {
        let site = classify_sync(path);
        if let Some(kind) = self.take_fault(site) {
            match kind {
                FaultKind::SyncFail
                | FaultKind::Enospc
                | FaultKind::Eio
                | FaultKind::Interrupt
                | FaultKind::RenameFail => return Err(fault_io(&FaultKind::SyncFail)),
                FaultKind::ShortWrite { .. } => {}
            }
        }
        if let Some(kind) = self.take_fault(FaultSite::WalSync) {
            if matches!(
                kind,
                FaultKind::SyncFail | FaultKind::Enospc | FaultKind::Eio | FaultKind::Interrupt
            ) {
                return Err(fault_io(&FaultKind::SyncFail));
            }
        }
        self.inner.sync_path(path)
    }

    fn sync_dir(&self, path: &Path) -> io::Result<()> {
        if let Some(kind) = self.take_fault(FaultSite::DirSync) {
            return Err(fault_io(&kind));
        }
        self.inner.sync_dir(path)
    }

    fn ensure_file(&self, path: &Path) -> io::Result<()> {
        self.inner.ensure_file(path)
    }

    fn try_lock_exclusive(&self, path: &Path) -> Result<FsLock, BackendError> {
        if let Some(kind) = self.take_fault(FaultSite::LockAcquire) {
            return Err(io_to_backend(fault_io(&kind), "lock acquire"));
        }
        self.inner.try_lock_exclusive(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use tempfile::tempdir;

    #[test]
    fn short_write_leaves_prefix_and_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wal").join("000001.log");
        let fs = FaultInjectingFs::new(OsFileSystem::shared());
        fs.inject_once(FaultSite::WalAppend, FaultKind::ShortWrite { bytes: 3 });
        let err = fs.append(&path, b"abcdef").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        assert_eq!(fs.read(&path).unwrap(), b"abc");
    }

    #[test]
    fn short_write_truncate_does_not_report_ok() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("MANIFEST-000001");
        let fs = FaultInjectingFs::new(OsFileSystem::shared());
        fs.write_truncate(&target, b"full-body-contents").unwrap();
        fs.inject_once(FaultSite::ManifestWrite, FaultKind::ShortWrite { bytes: 4 });
        let err = fs
            .write_truncate(
                &dir.path().join(".MANIFEST-000002.tmp"),
                b"abcdefghijklmnop",
            )
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        // Original published tip untouched.
        assert_eq!(fs.read(&target).unwrap(), b"full-body-contents");
    }

    #[test]
    fn enospc_maps_to_quota() {
        let err = io_to_backend(io::Error::new(io::ErrorKind::StorageFull, "x"), "append");
        assert!(matches!(err, BackendError::QuotaExceeded { .. }));
    }
}
