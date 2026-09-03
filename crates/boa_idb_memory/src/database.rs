//! In-memory database implementation.

use std::sync::Arc;

use boa_idb_core::backend::capabilities::BackendCapabilities;
use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendTxn, Database};
use boa_idb_core::backend::types::DatabaseMeta;
use boa_idb_core::proto::{Durability, StoreId, TxnMode};
use parking_lot::RwLock;

use crate::storage::StorageState;
use crate::txn::MemoryTxn;

/// In-memory database implementation.
pub struct MemoryDatabase {
    meta: DatabaseMeta,
    storage_state: Arc<RwLock<StorageState>>,
}

impl MemoryDatabase {
    /// Creates a new memory database.
    pub fn new(meta: DatabaseMeta, storage_state: Arc<RwLock<StorageState>>) -> Self {
        Self {
            meta,
            storage_state,
        }
    }
}

impl Database for MemoryDatabase {
    fn metadata(&self) -> &DatabaseMeta {
        &self.meta
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            snapshot_isolation: true,
            durable: false,
            concurrent: true,
            max_key_size: None,
            max_value_size: None,
        }
    }

    fn begin(
        &mut self,
        mode: TxnMode,
        scope: &[StoreId],
        _durability: Durability,
    ) -> Result<Box<dyn BackendTxn + '_>, BackendError> {
        Ok(Box::new(MemoryTxn::new(
            mode,
            scope.to_vec(),
            self.meta.clone(),
            self.storage_state.clone(),
        )))
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        Ok(())
    }

    fn close(self: Box<Self>) -> Result<(), BackendError> {
        Ok(())
    }
}
