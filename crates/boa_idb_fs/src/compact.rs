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
/// On success: new segment + manifest/`CURRENT` published, in-memory tip updated
/// to match disk, then superseded WAL best-effort removed. Mid-failure before
/// `CURRENT` publish leaves the previous generation; after publish the in-memory
/// tip tracks the new generation even if `meta.scf` sync fails (open heals meta).
/// Callers must not treat compaction `Err` as a failed user transaction once WAL
/// commit succeeded; prefer retry on the next write.
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
    let old_wal_seq = state.wal_seq;
    let new_seg_seq = state
        .segments
        .iter()
        .map(|s| s.seq)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let new_manifest_seq = state.manifest_seq.saturating_add(1);
    let new_wal_seq = old_wal_seq.saturating_add(1);

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

    // 4. Memory must match the published tip immediately. Otherwise a later
    //    meta/cleanup failure plus ignored compact Err leaves commits appending
    //    to the superseded WAL while CURRENT points at an empty generation.
    let old_segments = std::mem::take(&mut state.segments);
    for old in &old_segments {
        old.mark_reclaimable();
    }
    state.segments = vec![guard];
    state.manifest_seq = new_manifest_seq;
    state.wal_seq = new_wal_seq;
    state.wal_bytes_since_compact = 0;
    state.wal_frames_since_compact = 0;
    drop(old_segments);

    // 5. Align meta.scf (open heals mismatch if this fails).
    let meta_err = if let Some(meta) = &state.meta {
        write_meta_file(db_dir, meta, true, fs).err()
    } else {
        None
    };

    // 6. Best-effort remove superseded WAL (data now lives in the segment).
    let old_wal = wal_path(db_dir, old_wal_seq);
    let _ = fs.remove_file(&old_wal);

    if let Some(err) = meta_err {
        return Err(err);
    }
    Ok(())
}
