//! Shared mutable/immutable database maps and segment ownership (M6-B1).

use archery::shared_pointer::kind::ArcK;
use boa_idb_core::backend::types::DatabaseMeta;
use boa_idb_core::proto::{IndexId, StoreId};
use rpds::RedBlackTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Key for a record: (store_id, encoded_key_bytes).
pub(crate) type RecordKey = (StoreId, Vec<u8>);

/// Key for an index entry: (index_id, index_key_bytes, primary_key_bytes).
pub(crate) type IndexKey = (IndexId, Vec<u8>, Vec<u8>);

/// Thread-safe persistent ordered map (structural clone) for records/indexes.
pub(crate) type PersistMap<K, V> = RedBlackTreeMap<K, V, ArcK>;

fn empty_persist_map<K: Ord + Clone, V: Clone>() -> PersistMap<K, V> {
    PersistMap::new_with_ptr_kind()
}

/// Default WAL size that triggers compaction (R8.3.3).
pub const DEFAULT_WAL_COMPACT_BYTES: u64 = 64 * 1024 * 1024;
/// Default committed frame-group count that triggers compaction.
pub const DEFAULT_WAL_COMPACT_FRAMES: u64 = 10_000;

/// Counts structural snapshot clones for O(1)/O(log n) proofs (R8.3.4).
#[derive(Debug, Default)]
pub struct SnapshotMeter {
    /// Number of structural persistent-map clones performed for readonly begins.
    pub structural_clones: AtomicU64,
    /// Number of readonly snapshot begins.
    pub snapshot_begins: AtomicU64,
    /// Deep per-record walks (must stay 0 on the RO begin path).
    pub deep_record_walks: AtomicU64,
}

impl SnapshotMeter {
    /// Creates a shared meter for tests/factory injection.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Records a structural map clone (not an O(n) record copy).
    pub fn record_structural_clone(&self) {
        self.structural_clones.fetch_add(1, Ordering::SeqCst);
    }

    /// Records a readonly transaction begin.
    pub fn record_snapshot_begin(&self) {
        self.snapshot_begins.fetch_add(1, Ordering::SeqCst);
    }

    /// Records an accidental deep walk (tests assert this stays zero).
    pub fn record_deep_walk(&self, records_visited: u64) {
        self.deep_record_walks
            .fetch_add(records_visited, Ordering::SeqCst);
    }

    /// Snapshot of counters.
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.structural_clones.load(Ordering::SeqCst),
            self.snapshot_begins.load(Ordering::SeqCst),
            self.deep_record_walks.load(Ordering::SeqCst),
        )
    }
}

/// Keeps a segment file alive while any generation/snapshot holds an `Arc`.
///
/// Only segments marked reclaimable (superseded by compaction) are unlinked
/// when the last `Arc` drops. The live published tip must never use reclaim.
pub struct SegmentGuard {
    /// Segment sequence number.
    pub seq: u64,
    path: PathBuf,
    reclaim_on_drop: std::sync::atomic::AtomicBool,
    fs: Arc<dyn crate::vfs::FileSystem>,
}

impl std::fmt::Debug for SegmentGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SegmentGuard")
            .field("seq", &self.seq)
            .field("path", &self.path)
            .field(
                "reclaim_on_drop",
                &self
                    .reclaim_on_drop
                    .load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

impl SegmentGuard {
    /// Claims a live published segment (file must outlive the process handle).
    pub fn adopt_live(seq: u64, path: PathBuf, fs: Arc<dyn crate::vfs::FileSystem>) -> Arc<Self> {
        Arc::new(Self {
            seq,
            path,
            reclaim_on_drop: std::sync::atomic::AtomicBool::new(false),
            fs,
        })
    }

    /// Absolute path of the segment file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Marks this file for deletion when the last snapshot releases it.
    pub fn mark_reclaimable(&self) {
        self.reclaim_on_drop
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for SegmentGuard {
    fn drop(&mut self) {
        if self
            .reclaim_on_drop
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            let _ = self.fs.remove_file(&self.path);
        }
    }
}

/// Mutable database state held in memory while the DB is open.
#[derive(Debug, Clone)]
pub(crate) struct DbState {
    /// Database metadata.
    pub meta: Option<DatabaseMeta>,
    /// Primary records (persistent map — structural clone is O(1)).
    pub records: PersistMap<RecordKey, Vec<u8>>,
    /// Index entries.
    pub index_entries: PersistMap<IndexKey, ()>,
    /// Key generators.
    pub key_generators: HashMap<StoreId, f64>,
    /// Next WAL transaction sequence.
    pub next_txn_seq: u64,
    /// Manifest sequence counter.
    pub manifest_seq: u64,
    /// Active WAL file sequence (`wal/<seq>.log`).
    pub wal_seq: u64,
    /// Bytes appended to the current WAL since last compaction.
    pub wal_bytes_since_compact: u64,
    /// Committed frame groups since last compaction.
    pub wal_frames_since_compact: u64,
    /// Live segment files retained by this generation.
    pub segments: Vec<Arc<SegmentGuard>>,
}

impl Default for DbState {
    fn default() -> Self {
        Self {
            meta: None,
            records: empty_persist_map(),
            index_entries: empty_persist_map(),
            key_generators: HashMap::new(),
            next_txn_seq: 1,
            manifest_seq: 1,
            wal_seq: 1,
            wal_bytes_since_compact: 0,
            wal_frames_since_compact: 0,
            segments: Vec::new(),
        }
    }
}

impl DbState {
    /// Structurally clones maps for a readonly snapshot, metering the cost class.
    pub fn snapshot_clone(&self, meter: &SnapshotMeter) -> Self {
        meter.record_snapshot_begin();
        meter.record_structural_clone();
        // RedBlackTreeMap::clone is structural. Do not iterate records here.
        Self {
            meta: self.meta.clone(),
            records: self.records.clone(),
            index_entries: self.index_entries.clone(),
            key_generators: self.key_generators.clone(),
            next_txn_seq: self.next_txn_seq,
            manifest_seq: self.manifest_seq,
            wal_seq: self.wal_seq,
            wal_bytes_since_compact: self.wal_bytes_since_compact,
            wal_frames_since_compact: self.wal_frames_since_compact,
            // Share segment guards so compaction cannot delete under this snapshot.
            segments: self.segments.clone(),
        }
    }

    /// Counts primary keys currently resident in memory.
    pub fn key_count(&self) -> u64 {
        self.records.size() as u64
    }

    /// Approximate usage estimate for `Storage::usage_bytes`.
    pub fn usage_bytes(&self) -> u64 {
        let mut total = 256u64;
        for (k, v) in self.records.iter() {
            total += (8 + k.1.len() + v.len()) as u64;
        }
        for (k, ()) in self.index_entries.iter() {
            total += (8 + k.1.len() + k.2.len()) as u64;
        }
        total
    }
}
