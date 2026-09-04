//! WAL compaction into immutable segments (R8.3.3).

use crate::atomic::{ensure_file, truncate_file};
use crate::meta::{ManifestData, write_manifest, write_meta_file};
use crate::segment::write_segment_file;
use crate::state::DbState;
use crate::vfs::FileSystem;
use boa_idb_core::backend::error::BackendError;
use std::path::Path;
use std::sync::Arc;

/// Compaction thresholds (production defaults from R8.3.3).
#[derive(Debug, Clone, Copy)]
pub struct CompactConfig {
    /// Compact when WAL bytes since last compact reach this size.
    pub wal_bytes: u64,
    /// Compact when committed frame groups since last compact reach this count.
    pub wal_frames: u64,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            wal_bytes: crate::state::DEFAULT_WAL_COMPACT_BYTES,
            wal_frames: crate::state::DEFAULT_WAL_COMPACT_FRAMES,
        }
    }
}

impl CompactConfig {
    /// Returns true when either threshold is met.
    pub(crate) fn should_compact(self, state: &DbState) -> bool {
        state.wal_bytes_since_compact >= self.wal_bytes
            || state.wal_frames_since_compact >= self.wal_frames
    }
}

/// Path of the active WAL file for `state.wal_seq`.
pub fn wal_path(db_dir: &Path, wal_seq: u64) -> std::path::PathBuf {
    db_dir.join("wal").join(format!("{wal_seq:06}.log"))
}

/// Runs compaction if thresholds are met.
///
/// On success: new segment + manifest/`CURRENT` published, then WAL rotated to
/// an empty generation. Mid-failure leaves either the previous or the new
/// fully published generation (never a hybrid readable state). Committed
/// in-memory data is left intact even if compaction fails — callers must not
/// treat compaction `Err` as a failed user transaction once WAL commit
/// succeeded; prefer logging/retry on the next write.
pub fn maybe_compact(
    db_dir: &Path,
    state: &mut DbState,
    cfg: CompactConfig,
    fs: &Arc<dyn FileSystem>,
) -> Result<bool, BackendError> {
    if !cfg.should_compact(state) {
        return Ok(false);
    }
    compact_now(db_dir, state, fs)?;
    Ok(true)
}

/// Unconditional compaction (tests).
pub fn compact_now(
    db_dir: &Path,
    state: &mut DbState,
    fs: &Arc<dyn FileSystem>,
) -> Result<(), BackendError> {
    let new_seg_seq = state
        .segments
        .iter()
        .map(|s| s.seq)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let new_manifest_seq = state.manifest_seq.saturating_add(1);
    let new_wal_seq = state.wal_seq.saturating_add(1);

    // 1. Durable segment with full state (includes WAL-applied data).
    let guard = write_segment_file(db_dir, new_seg_seq, state, true, fs)?;

    // 2. Prepare empty next WAL before publishing CURRENT.
    let new_wal = wal_path(db_dir, new_wal_seq);
    ensure_file(&new_wal, fs)?;
    truncate_file(&new_wal, 0, fs)?;

    // 3. Publish manifest + CURRENT (only after segment+empty WAL exist).
    let manifest = ManifestData {
        manifest_seq: new_manifest_seq,
        wal_seq: new_wal_seq,
        segments: vec![new_seg_seq],
    };
    write_manifest(db_dir, &manifest, true, fs)?;

    // 4. Persist meta.scf aligned with compacted state.
    if let Some(meta) = &state.meta {
        write_meta_file(db_dir, meta, true, fs)?;
    }

    // 5. Replace live segment set (old Arcs may remain in readonly snapshots).
    for old in std::mem::take(&mut state.segments) {
        old.mark_reclaimable();
    }
    state.segments = vec![guard];
    state.manifest_seq = new_manifest_seq;
    state.wal_seq = new_wal_seq;
    state.wal_bytes_since_compact = 0;
    state.wal_frames_since_compact = 0;
    Ok(())
}
