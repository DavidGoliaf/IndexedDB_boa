//! IdbEngine — central bridge between L1 JS bindings and L2 core engine.
//!
//! Owns the backend storage, database registry, and transaction scheduler.
//! All API methods delegate to this engine.

use boa_idb_core::backend::traits::{BackendFactory, Database, Storage};
use boa_idb_core::backend::types::{DatabaseMeta, IndexSpec, StoreMeta, StoreSpec};
use boa_idb_core::clone::encode::encode_scf;
use boa_idb_core::clone::scvalue::ScValue;
use boa_idb_core::engine::keygen::KeyGenerator;
use boa_idb_core::engine::open_queue::{OpenQueue, OpenQueueAction};
use boa_idb_core::engine::registry::Registry;
use boa_idb_core::engine::request::{Request, RequestState};
use boa_idb_core::engine::scheduler::{TransactionScheduler, TxnQueueItem};
use boa_idb_core::engine::transaction::{Transaction, TxnState};
use boa_idb_core::error::IdbError;
use boa_idb_core::key::encode::encode_key;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use boa_idb_core::proto::{
    ConnectionId, CursorId, Direction, Durability, IndexId, OpOutcome, Operation, RecordSnapshot,
    RequestId, SourceRef, StorageKey, StoreId, TxnId, TxnMode,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Result of opening a database connection.
pub struct OpenResult {
    pub connection_id: ConnectionId,
    pub version: u64,
    pub needs_upgrade: bool,
    pub old_version: u64,
}

/// Result of executing a request.
pub struct RequestResult {
    pub request_id: RequestId,
    pub outcome: OpOutcome,
}

/// Central engine bridging L1 bindings to L2 core.
pub struct IdbEngine {
    pub backend_factory: Arc<dyn BackendFactory>,
    pub storage: Box<dyn Storage>,
    pub registry: Registry,
    pub scheduler: TransactionScheduler,
    pub open_queues: HashMap<String, OpenQueue>,
    pub connections: HashMap<ConnectionId, ConnectionState>,
    pub transactions: HashMap<TxnId, TransactionState>,
    pub requests: HashMap<RequestId, Request>,
    pub key_generators: HashMap<(TxnId, StoreId), KeyGenerator>,
    pub limit_config: LimitConfig,
    next_connection_id: ConnectionId,
    next_txn_id: TxnId,
    next_request_id: RequestId,
}

pub struct ConnectionState {
    pub id: ConnectionId,
    pub database_name: String,
    pub version: u64,
    pub open: bool,
}

pub struct TransactionState {
    pub id: TxnId,
    pub connection_id: ConnectionId,
    pub mode: TxnMode,
    pub scope: Vec<StoreId>,
    pub durability: Durability,
    pub active: bool,
    pub request_queue: Vec<RequestId>,
    pub completed: bool,
}

impl IdbEngine {
    pub fn new(backend_factory: Arc<dyn BackendFactory>, storage_key: &StorageKey) -> Self {
        let storage = backend_factory
            .open_storage(storage_key)
            .expect("Failed to open storage");

        Self {
            backend_factory,
            storage,
            registry: Registry::new(),
            scheduler: TransactionScheduler::new(),
            open_queues: HashMap::new(),
            connections: HashMap::new(),
            transactions: HashMap::new(),
            requests: HashMap::new(),
            key_generators: HashMap::new(),
            limit_config: LimitConfig::default(),
            next_connection_id: 1,
            next_txn_id: 1,
            next_request_id: 1,
        }
    }

    fn alloc_connection_id(&mut self) -> ConnectionId {
        let id = self.next_connection_id;
        self.next_connection_id += 1;
        id
    }

    fn alloc_txn_id(&mut self) -> TxnId {
        let id = self.next_txn_id;
        self.next_txn_id += 1;
        id
    }

    fn alloc_request_id(&mut self) -> RequestId {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    // ── Database open/close ──────────────────────────────────────────────

    pub fn open_database(
        &mut self,
        name: &str,
        requested_version: u64,
    ) -> Result<OpenResult, IdbError> {
        // Allocate request ID before mutable borrow of open_queues
        let request_id = self.alloc_request_id();

        // Get or create open queue
        let queue = self.open_queues.entry(name.to_string()).or_insert_with(|| {
            let current_version = self.registry.get(name).map_or(0, |m| m.version);
            OpenQueue::new(name.to_string(), current_version)
        });

        queue.enqueue_open(request_id, requested_version);

        match queue.process_next() {
            Some(OpenQueueAction::OpenConnection {
                request_id,
                version,
            }) => {
                let conn_id = self.alloc_connection_id();
                self.connections.insert(
                    conn_id,
                    ConnectionState {
                        id: conn_id,
                        database_name: name.to_string(),
                        version,
                        open: true,
                    },
                );
                self.storage
                    .open_database(name)
                    .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;
                self.registry.add_connection(name);

                Ok(OpenResult {
                    connection_id: conn_id,
                    version,
                    needs_upgrade: false,
                    old_version: version,
                })
            }
            Some(OpenQueueAction::StartUpgrade {
                request_id,
                old_version,
                new_version,
            }) => {
                let conn_id = self.alloc_connection_id();
                self.connections.insert(
                    conn_id,
                    ConnectionState {
                        id: conn_id,
                        database_name: name.to_string(),
                        version: new_version,
                        open: true,
                    },
                );
                self.storage
                    .open_database(name)
                    .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;
                self.registry.add_connection(name);

                Ok(OpenResult {
                    connection_id: conn_id,
                    version: new_version,
                    needs_upgrade: true,
                    old_version,
                })
            }
            Some(OpenQueueAction::FailOpen { request_id, error }) => Err(error),
            Some(OpenQueueAction::SendBlocked { request_id }) => {
                // Connection is blocked — for now, return an error
                // In a full implementation, this would fire onblocked
                Err(IdbError::Unknown(
                    "Database open blocked by existing connections".into(),
                ))
            }
            None => Err(IdbError::Unknown("Open queue is empty".into())),
            _ => Err(IdbError::Unknown("Unexpected open queue action".into())),
        }
    }

    pub fn close_connection(&mut self, conn_id: ConnectionId) {
        if let Some(conn) = self.connections.get_mut(&conn_id) {
            conn.open = false;
            self.registry.remove_connection(&conn.database_name);
            // Unblock open queue if applicable
            if let Some(queue) = self.open_queues.get_mut(&conn.database_name) {
                queue.on_connection_closed();
            }
        }
    }

    pub fn delete_database(&mut self, name: &str) -> Result<(), IdbError> {
        self.storage
            .delete_database(name)
            .map_err(|e| IdbError::Unknown(format!("Failed to delete database: {e}")))?;
        self.registry.unregister(name);
        self.open_queues.remove(name);
        Ok(())
    }

    pub fn list_databases(&self) -> Result<Vec<(String, u64)>, IdbError> {
        self.storage
            .list_databases()
            .map_err(|e| IdbError::Unknown(format!("Failed to list databases: {e}")))
    }

    // ── Schema operations (VersionChange) ────────────────────────────────

    pub fn create_object_store(
        &mut self,
        txn_id: TxnId,
        name: &str,
        key_path: Option<boa_idb_core::key::path::KeyPath>,
        auto_increment: bool,
    ) -> Result<StoreId, IdbError> {
        let txn_state = self
            .transactions
            .get(&txn_id)
            .ok_or_else(|| IdbError::Unknown("Transaction not found".into()))?;
        if txn_state.mode != TxnMode::VersionChange {
            return Err(IdbError::InvalidState(
                "createObjectStore requires a versionchange transaction".into(),
            ));
        }

        let conn_id = txn_state.connection_id;
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        let db_name = conn.database_name.clone();

        let mut db = self
            .storage
            .open_database(&db_name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;

        let spec = StoreSpec {
            name: name.into(),
            key_path: key_path.unwrap_or(boa_idb_core::key::path::KeyPath::Empty),
            auto_increment,
        };

        let store_id = {
            let mut txn = db
                .begin(TxnMode::VersionChange, &[], Durability::Default)
                .map_err(|e| IdbError::Unknown(format!("Failed to begin txn: {e}")))?;
            let sid = txn
                .create_store(&spec)
                .map_err(|e| IdbError::Unknown(format!("Failed to create store: {e}")))?;
            txn.commit()
                .map_err(|e| IdbError::Unknown(format!("Failed to commit: {e}")))?;
            sid
        };

        // Update registry
        if let Some(meta) = self.registry.get_mut(&db_name) {
            meta.stores.push(StoreMeta {
                id: store_id,
                name: name.into(),
                key_path: spec.key_path,
                auto_increment,
                key_gen: 0.0,
                indexes: Vec::new(),
                deleted: false,
            });
        }

        Ok(store_id)
    }

    pub fn create_index(
        &mut self,
        txn_id: TxnId,
        store_id: StoreId,
        name: &str,
        key_path: boa_idb_core::key::path::KeyPath,
        unique: bool,
        multi_entry: bool,
    ) -> Result<IndexId, IdbError> {
        let txn_state = self
            .transactions
            .get(&txn_id)
            .ok_or_else(|| IdbError::Unknown("Transaction not found".into()))?;
        if txn_state.mode != TxnMode::VersionChange {
            return Err(IdbError::InvalidState(
                "createIndex requires a versionchange transaction".into(),
            ));
        }

        let conn_id = txn_state.connection_id;
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        let db_name = conn.database_name.clone();

        let mut db = self
            .storage
            .open_database(&db_name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;

        let spec = IndexSpec {
            name: name.into(),
            key_path,
            unique,
            multi_entry,
        };

        let index_id = {
            let mut txn = db
                .begin(TxnMode::VersionChange, &[store_id], Durability::Default)
                .map_err(|e| IdbError::Unknown(format!("Failed to begin txn: {e}")))?;
            let iid = txn
                .create_index(store_id, &spec)
                .map_err(|e| IdbError::Unknown(format!("Failed to create index: {e}")))?;
            txn.commit()
                .map_err(|e| IdbError::Unknown(format!("Failed to commit: {e}")))?;
            iid
        };

        // Update registry
        if let Some(meta) = self.registry.get_mut(&db_name) {
            if let Some(store) = meta.stores.iter_mut().find(|s| s.id == store_id) {
                store.indexes.push(boa_idb_core::backend::types::IndexMeta {
                    id: index_id,
                    store_id,
                    name: name.into(),
                    key_path: spec.key_path,
                    unique,
                    multi_entry,
                    deleted: false,
                });
            }
        }

        Ok(index_id)
    }

    // ── Transaction management ───────────────────────────────────────────

    pub fn begin_transaction(
        &mut self,
        conn_id: ConnectionId,
        mode: TxnMode,
        store_names: &[&str],
        durability: Durability,
    ) -> Result<TxnId, IdbError> {
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        if !conn.open {
            return Err(IdbError::InvalidState("Connection is closed".into()));
        }
        let db_name = conn.database_name.clone();

        // Resolve store names to IDs
        let meta = self
            .registry
            .get(&db_name)
            .ok_or_else(|| IdbError::Unknown("Database not found in registry".into()))?;

        let scope: Vec<StoreId> = store_names
            .iter()
            .map(|name| {
                meta.stores
                    .iter()
                    .find(|s| s.name.to_string() == *name && !s.deleted)
                    .map(|s| s.id)
                    .ok_or_else(|| IdbError::NotFound(format!("Object store '{name}' not found")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let txn_id = self.alloc_txn_id();

        // Enqueue in scheduler
        self.scheduler.enqueue(TxnQueueItem {
            id: txn_id,
            mode,
            scope: scope.clone(),
        });

        self.transactions.insert(
            txn_id,
            TransactionState {
                id: txn_id,
                connection_id: conn_id,
                mode,
                scope,
                durability,
                active: true,
                request_queue: Vec::new(),
                completed: false,
            },
        );

        // For simplicity, start the transaction immediately
        // In a full implementation, the scheduler would control this
        let _ready = self.scheduler.poll_ready();

        Ok(txn_id)
    }

    pub fn is_transaction_active(&self, txn_id: TxnId) -> bool {
        self.transactions
            .get(&txn_id)
            .is_some_and(|t| t.active && !t.completed)
    }

    pub fn deactivate_transaction(&mut self, txn_id: TxnId) {
        if let Some(txn) = self.transactions.get_mut(&txn_id) {
            txn.active = false;
        }
    }

    pub fn activate_transaction(&mut self, txn_id: TxnId) {
        if let Some(txn) = self.transactions.get_mut(&txn_id) {
            txn.active = true;
        }
    }

    pub fn commit_transaction(&mut self, txn_id: TxnId) -> Result<(), IdbError> {
        if let Some(txn) = self.transactions.get_mut(&txn_id) {
            txn.completed = true;
            txn.active = false;
        }
        self.scheduler.on_txn_finished(txn_id);
        Ok(())
    }

    pub fn abort_transaction(&mut self, txn_id: TxnId) -> Result<(), IdbError> {
        if let Some(txn) = self.transactions.get_mut(&txn_id) {
            txn.completed = true;
            txn.active = false;
        }
        self.scheduler.on_txn_finished(txn_id);
        Ok(())
    }

    // ── Data operations ──────────────────────────────────────────────────

    pub fn put(
        &mut self,
        txn_id: TxnId,
        store_id: StoreId,
        value: &mut ScValue,
        explicit_key: Option<&Key>,
        no_overwrite: bool,
    ) -> Result<Key, IdbError> {
        let txn_state = self
            .transactions
            .get(&txn_id)
            .ok_or_else(|| IdbError::Unknown("Transaction not found".into()))?;
        if !txn_state.active {
            return Err(IdbError::TransactionInactive);
        }

        let conn_id = txn_state.connection_id;
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        let db_name = conn.database_name.clone();

        let meta = self
            .registry
            .get(&db_name)
            .ok_or_else(|| IdbError::Unknown("Database not found".into()))?;
        let store_meta = meta
            .stores
            .iter()
            .find(|s| s.id == store_id)
            .ok_or_else(|| IdbError::NotFound("Store not found".into()))?;

        let mut db = self
            .storage
            .open_database(&db_name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;

        let keygen = self
            .key_generators
            .entry((txn_id, store_id))
            .or_insert_with(|| KeyGenerator::new(store_meta.key_gen));

        let mut txn = db
            .begin(txn_state.mode, &txn_state.scope, txn_state.durability)
            .map_err(|e| IdbError::Unknown(format!("Failed to begin txn: {e}")))?;

        let result = boa_idb_core::engine::ops_store::put(
            txn.as_mut(),
            store_meta,
            keygen,
            value,
            explicit_key,
            no_overwrite,
            &self.limit_config,
        )?;

        txn.commit()
            .map_err(|e| IdbError::Unknown(format!("Failed to commit: {e}")))?;

        Ok(result.key)
    }

    pub fn get(
        &mut self,
        txn_id: TxnId,
        source: SourceRef,
        key: &Key,
    ) -> Result<Option<ScValue>, IdbError> {
        let txn_state = self
            .transactions
            .get(&txn_id)
            .ok_or_else(|| IdbError::Unknown("Transaction not found".into()))?;
        if !txn_state.active {
            return Err(IdbError::TransactionInactive);
        }

        let conn_id = txn_state.connection_id;
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        let db_name = conn.database_name.clone();

        let mut db = self
            .storage
            .open_database(&db_name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;

        let mut txn = db
            .begin(txn_state.mode, &txn_state.scope, txn_state.durability)
            .map_err(|e| IdbError::Unknown(format!("Failed to begin txn: {e}")))?;

        let store_id = match source {
            SourceRef::Store(id) => id,
            SourceRef::Index { store, .. } => store,
        };

        let mut encoded_key = Vec::new();
        encode_key(key, &mut encoded_key, &self.limit_config)?;

        let raw = txn
            .get(store_id, &encoded_key)
            .map_err(|e| IdbError::Unknown(format!("Backend error: {e}")))?;

        txn.commit()
            .map_err(|e| IdbError::Unknown(format!("Failed to commit: {e}")))?;

        match raw {
            Some(bytes) => {
                let sc_value = boa_idb_core::clone::decode::decode_scf(&bytes, &self.limit_config)
                    .map_err(|e| IdbError::DataClone(format!("Failed to decode value: {e}")))?;
                Ok(Some(sc_value))
            }
            None => Ok(None),
        }
    }

    pub fn delete(
        &mut self,
        txn_id: TxnId,
        store_id: StoreId,
        key: &Key,
    ) -> Result<bool, IdbError> {
        let txn_state = self
            .transactions
            .get(&txn_id)
            .ok_or_else(|| IdbError::Unknown("Transaction not found".into()))?;
        if !txn_state.active {
            return Err(IdbError::TransactionInactive);
        }

        let conn_id = txn_state.connection_id;
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        let db_name = conn.database_name.clone();

        let mut db = self
            .storage
            .open_database(&db_name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;

        let mut txn = db
            .begin(txn_state.mode, &txn_state.scope, txn_state.durability)
            .map_err(|e| IdbError::Unknown(format!("Failed to begin txn: {e}")))?;

        let mut encoded_key = Vec::new();
        encode_key(key, &mut encoded_key, &self.limit_config)?;

        let existed = txn
            .delete_record(store_id, &encoded_key)
            .map_err(|e| IdbError::Unknown(format!("Backend error: {e}")))?;

        txn.commit()
            .map_err(|e| IdbError::Unknown(format!("Failed to commit: {e}")))?;

        Ok(existed)
    }

    pub fn clear(&mut self, txn_id: TxnId, store_id: StoreId) -> Result<(), IdbError> {
        let txn_state = self
            .transactions
            .get(&txn_id)
            .ok_or_else(|| IdbError::Unknown("Transaction not found".into()))?;
        if !txn_state.active {
            return Err(IdbError::TransactionInactive);
        }

        let conn_id = txn_state.connection_id;
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        let db_name = conn.database_name.clone();

        let mut db = self
            .storage
            .open_database(&db_name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;

        let mut txn = db
            .begin(txn_state.mode, &txn_state.scope, txn_state.durability)
            .map_err(|e| IdbError::Unknown(format!("Failed to begin txn: {e}")))?;

        txn.clear(store_id)
            .map_err(|e| IdbError::Unknown(format!("Backend error: {e}")))?;

        txn.commit()
            .map_err(|e| IdbError::Unknown(format!("Failed to commit: {e}")))?;

        Ok(())
    }

    pub fn count(
        &mut self,
        txn_id: TxnId,
        source: SourceRef,
        range: &EncodedRange,
    ) -> Result<u64, IdbError> {
        let txn_state = self
            .transactions
            .get(&txn_id)
            .ok_or_else(|| IdbError::Unknown("Transaction not found".into()))?;
        if !txn_state.active {
            return Err(IdbError::TransactionInactive);
        }

        let conn_id = txn_state.connection_id;
        let conn = self
            .connections
            .get(&conn_id)
            .ok_or_else(|| IdbError::Unknown("Connection not found".into()))?;
        let db_name = conn.database_name.clone();

        let mut db = self
            .storage
            .open_database(&db_name)
            .map_err(|e| IdbError::Unknown(format!("Failed to open database: {e}")))?;

        let mut txn = db
            .begin(txn_state.mode, &txn_state.scope, txn_state.durability)
            .map_err(|e| IdbError::Unknown(format!("Failed to begin txn: {e}")))?;

        let result = txn
            .count(source, range)
            .map_err(|e| IdbError::Unknown(format!("Backend error: {e}")))?;

        txn.commit()
            .map_err(|e| IdbError::Unknown(format!("Failed to commit: {e}")))?;

        Ok(result)
    }

    pub fn get_store_meta(&self, db_name: &str, store_id: StoreId) -> Option<&StoreMeta> {
        self.registry
            .get(db_name)
            .and_then(|meta| meta.stores.iter().find(|s| s.id == store_id))
    }

    pub fn get_db_meta(&self, db_name: &str) -> Option<&DatabaseMeta> {
        self.registry.get(db_name)
    }

    pub fn get_connection(&self, conn_id: ConnectionId) -> Option<&ConnectionState> {
        self.connections.get(&conn_id)
    }

    pub fn get_transaction(&self, txn_id: TxnId) -> Option<&TransactionState> {
        self.transactions.get(&txn_id)
    }

    pub fn cleanup_active_transactions(&mut self) {
        for txn in self.transactions.values_mut() {
            if !txn.completed {
                txn.active = false;
            }
        }
    }
}
