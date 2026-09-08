//! Filesystem backend for `IndexedDB` (M6-A/B1/B2/B3).
//!
//! Provides WAL-backed durability, advisory `LOCK`, immutable segments,
//! structural-share MVCC snapshots, a `FileSystem` fault-injection seam, and a
//! dedicated crash-worker binary for forced-kill recovery tests.

#![deny(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::similar_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::collapsible_if,
    clippy::suspicious_open_options,
    clippy::return_self_not_must_use,
    clippy::zero_sized_map_values,
    clippy::for_kv_map,
    clippy::ignored_unit_patterns,
    clippy::needless_pass_by_value,
    clippy::enum_variant_names,
    clippy::type_complexity,
    clippy::unnecessary_wraps,
    clippy::used_underscore_binding,
    clippy::explicit_iter_loop,
    dead_code
)]

mod apply;
mod atomic;
mod compact;
mod cursor;
mod database;
mod factory;
mod lock;
mod meta;
mod naming;
mod segment;
mod state;
mod storage;
mod sync_hooks;
mod txn;
mod vfs;
mod wal;

pub use compact::CompactConfig;
pub use factory::{DEFAULT_MAX_KEYS_IN_MEMORY, FsBackendFactory};
pub use naming::{database_dir_name, database_root, storage_dir_name, storage_root};
pub use state::{DEFAULT_WAL_COMPACT_BYTES, DEFAULT_WAL_COMPACT_FRAMES, SnapshotMeter};
pub use sync_hooks::{CountingSyncHooks, OsSyncHooks, SyncHooks};
pub use vfs::{FaultInjectingFs, FaultKind, FaultSite, FileSystem, OsFileSystem, SyncHooksFs};
pub use wal::{
    CodecError, FLAG_COMMIT, FLAG_CONTINUES, MAX_FRAME_PAYLOAD, RecoveredWal, WAL_MAGIC, WalFrame,
    WalOp, decode_frame, encode_frame, encode_txn_frames, encode_txn_frames_limited,
    recover_committed_frames,
};
