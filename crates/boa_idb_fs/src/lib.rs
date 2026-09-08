//! Filesystem backend for `IndexedDB` (M6-A foundation).
//!
//! Provides WAL-backed durability, advisory `LOCK`, atomic `CURRENT`/`MANIFEST`
//! updates and an in-memory ordered index rebuilt on open. Segment compaction
//! and O(1)/O(log n) MVCC snapshots are deferred to M6-B.

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
    dead_code
)]

mod apply;
mod atomic;
mod cursor;
mod database;
mod factory;
mod lock;
mod meta;
mod naming;
mod state;
mod storage;
mod sync_hooks;
mod txn;
mod wal;

pub use factory::{DEFAULT_MAX_KEYS_IN_MEMORY, FsBackendFactory};
pub use naming::{database_dir_name, database_root, storage_dir_name, storage_root};
pub use sync_hooks::{CountingSyncHooks, OsSyncHooks, SyncHooks};
pub use wal::{
    CodecError, FLAG_COMMIT, FLAG_CONTINUES, MAX_FRAME_PAYLOAD, RecoveredWal, WAL_MAGIC, WalFrame,
    WalOp, decode_frame, encode_frame, encode_txn_frames, encode_txn_frames_limited,
    recover_committed_frames,
};
