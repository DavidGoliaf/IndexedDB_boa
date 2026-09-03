//! IdbEngine — storage ownership for the L1 driver.
//!
//! The engine owns the backend [`Storage`] and hands out short-lived
//! [`Database`] handles. All transaction orchestration (queues, savepoints,
//! index maintenance, event dispatch) lives in [`crate::driver`], which keeps
//! per-transaction backend handles and schema snapshots in its `TxnHandle`s.
//! The pre-driver synchronous per-operation flow was removed: it opened and
//! committed a fresh backend transaction for every single operation, which
//! provided no transactionality at all.

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::traits::{Database, Storage};
use boa_idb_core::error::IdbError;
use boa_idb_core::limits::LimitConfig;
use boa_idb_core::proto::{ConnectionId, StorageKey};
use std::sync::Arc;

/// Central storage owner bridging L1 bindings to the backend.
pub struct IdbEngine {
    /// Factory that created [`Self::storage`].
    pub backend_factory: Arc<dyn BackendFactory>,
    /// The backend storage for this storage key.
    pub storage: Box<dyn Storage>,
    /// Codec and validation limits shared by the driver.
    pub limit_config: LimitConfig,
    /// Next connection id.
    next_connection_id: ConnectionId,
}

impl IdbEngine {
    /// Opens the backend storage for `storage_key`.
    pub fn new(
        backend_factory: Arc<dyn BackendFactory>,
        storage_key: &StorageKey,
    ) -> Result<Self, IdbError> {
        let storage = backend_factory
            .open_storage(storage_key)
            .map_err(|e| IdbError::Unknown(format!("Failed to open storage: {e}")))?;
        Ok(Self {
            backend_factory,
            storage,
            limit_config: LimitConfig::default(),
            next_connection_id: 1,
        })
    }

    /// Allocates a connection id for a new `IDBDatabase`.
    pub fn alloc_connection_id(&mut self) -> ConnectionId {
        let id = self.next_connection_id;
        self.next_connection_id += 1;
        id
    }

    /// Opens a backend database handle by name (creating version 0 on demand).
    pub fn open_db_handle(&mut self, name: &str) -> Result<Box<dyn Database>, IdbError> {
        self.storage
            .open_database(name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))
    }

    /// Deletes a database by name.
    pub fn delete_database(&mut self, name: &str) -> Result<(), IdbError> {
        self.storage
            .delete_database(name)
            .map_err(|e| IdbError::Unknown(format!("Failed to delete database: {e}")))
    }

    /// Returns the committed version of a database (0 when missing).
    pub fn db_version(&mut self, name: &str) -> Result<u64, IdbError> {
        Ok(self.open_db_handle(name)?.metadata().version)
    }

    /// Lists all databases with their versions.
    pub fn list_databases(&mut self) -> Result<Vec<(String, u64)>, IdbError> {
        self.storage
            .list_databases()
            .map_err(|e| IdbError::Unknown(format!("Failed to list databases: {e}")))
    }
}
