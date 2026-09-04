//! `SQLite` backend for `IndexedDB`.
//!
//! This backend stores data in `SQLite` databases with WAL mode,
//! connection pooling (1 writer slot + N readers, checkout model), blob
//! overflow for large values, and snapshot isolation for read-only
//! transactions.

#![deny(unsafe_code)]
// Scoped pedantic-lint tuning (same policy as `boa_idb_core`): the SQLite
// row/column world is full of integer casts and SQL string assembly; the
// lints below fire on every such line without pointing at real defects.
#![allow(
    clippy::bool_to_int_with_if,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::doc_markdown,
    clippy::elidable_lifetime_names,
    clippy::format_push_string,
    clippy::large_stack_arrays,
    clippy::needless_raw_string_hashes,
    clippy::redundant_closure_for_method_calls,
    clippy::single_match_else
)]

mod blob;
pub mod cursor;
mod database;
mod factory;
pub mod naming;
mod pool;
pub mod schema;
mod storage;
mod txn;

pub use factory::SqliteBackendFactory;
pub use naming::storage_dir_name;
pub use storage::SqliteStorage;
