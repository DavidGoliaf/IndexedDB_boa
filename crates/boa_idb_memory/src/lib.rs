//! In-memory backend for `IndexedDB`.
//!
//! This backend stores all data in `BTreeMap` structures in memory.
//! It supports snapshot isolation via undo logs and is suitable for testing.

#![deny(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::enum_variant_names,
    clippy::elidable_lifetime_names,
    clippy::redundant_closure_for_method_calls,
    clippy::type_complexity,
    clippy::zero_sized_map_values,
    clippy::for_kv_map,
    clippy::ignored_unit_patterns,
    clippy::collapsible_if
)]

mod cursor;
mod database;
mod storage;
mod txn;

pub use storage::MemoryStorage;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendFactory, Storage};
use boa_idb_core::proto::StorageKey;

/// Factory for creating in-memory storage instances.
#[derive(Debug, Default)]
pub struct MemoryBackendFactory;

impl MemoryBackendFactory {
    /// Creates a new memory backend factory.
    pub fn new() -> Self {
        Self
    }
}

impl BackendFactory for MemoryBackendFactory {
    fn open_storage(&self, _key: &StorageKey) -> Result<Box<dyn Storage>, BackendError> {
        Ok(Box::new(MemoryStorage::new()))
    }
}
