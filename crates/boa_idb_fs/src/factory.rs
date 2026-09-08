//! Factory for the filesystem IndexedDB backend.

use std::path::PathBuf;
use std::sync::Arc;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendFactory, Storage};
use boa_idb_core::proto::StorageKey;

use crate::compact::CompactConfig;
use crate::naming::storage_root;
use crate::state::{DEFAULT_WAL_COMPACT_BYTES, DEFAULT_WAL_COMPACT_FRAMES, SnapshotMeter};
use crate::storage::FsStorage;
use crate::sync_hooks::SyncHooks;
use crate::vfs::{FileSystem, OsFileSystem, SyncHooksFs};
use crate::wal::MAX_FRAME_PAYLOAD;

/// Default `max_keys_in_memory` (R8.3.5).
pub const DEFAULT_MAX_KEYS_IN_MEMORY: u64 = 5_000_000;

/// Filesystem backend factory.
pub struct FsBackendFactory {
    root: PathBuf,
    fs: Arc<dyn FileSystem>,
    max_keys_in_memory: u64,
    max_frame_payload: u32,
    compact: CompactConfig,
    meter: Arc<SnapshotMeter>,
}

impl FsBackendFactory {
    /// Creates a factory rooted at `root` with the OS filesystem.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            fs: OsFileSystem::shared(),
            max_keys_in_memory: DEFAULT_MAX_KEYS_IN_MEMORY,
            max_frame_payload: MAX_FRAME_PAYLOAD,
            compact: CompactConfig {
                wal_bytes: DEFAULT_WAL_COMPACT_BYTES,
                wal_frames: DEFAULT_WAL_COMPACT_FRAMES,
            },
            meter: SnapshotMeter::new(),
        }
    }

    /// Overrides the filesystem seam (fault injection / test doubles).
    pub fn with_filesystem(mut self, fs: Arc<dyn FileSystem>) -> Self {
        self.fs = fs;
        self
    }

    /// Overrides sync hooks (tests inject counting observers).
    ///
    /// Implemented as [`SyncHooksFs`] over the OS filesystem.
    pub fn with_sync_hooks(mut self, hooks: Arc<dyn SyncHooks>) -> Self {
        self.fs = SyncHooksFs::new(hooks);
        self
    }

    /// Overrides the in-memory key limit.
    pub fn with_max_keys_in_memory(mut self, max: u64) -> Self {
        self.max_keys_in_memory = max;
        self
    }

    /// Overrides WAL frame payload limit (tests force multi-frame commits).
    pub fn with_max_frame_payload(mut self, max: u32) -> Self {
        self.max_frame_payload = max;
        self
    }

    /// Overrides compaction thresholds (tests use small values).
    pub fn with_compact_config(mut self, compact: CompactConfig) -> Self {
        self.compact = compact;
        self
    }

    /// Injects a shared snapshot meter for O(1) readonly proofs.
    pub fn with_snapshot_meter(mut self, meter: Arc<SnapshotMeter>) -> Self {
        self.meter = meter;
        self
    }

    /// Returns the factory snapshot meter.
    pub fn snapshot_meter(&self) -> Arc<SnapshotMeter> {
        self.meter.clone()
    }
}

impl BackendFactory for FsBackendFactory {
    fn open_storage(&self, key: &StorageKey) -> Result<Box<dyn Storage>, BackendError> {
        let path = storage_root(&self.root, &key.0);
        Ok(Box::new(FsStorage::new(
            path,
            key.0.clone(),
            self.fs.clone(),
            self.max_keys_in_memory,
            self.max_frame_payload,
            self.compact,
            self.meter.clone(),
        )?))
    }
}
