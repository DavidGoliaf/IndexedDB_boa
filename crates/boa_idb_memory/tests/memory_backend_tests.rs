//! Tests for the in-memory backend.

#![allow(clippy::float_cmp)]

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::proto::{Durability, TxnMode};
use boa_idb_memory::MemoryBackendFactory;

#[test]
fn test_factory_creates_storage() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key);
    assert!(storage.is_ok());
}

#[test]
fn test_storage_lists_empty() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let dbs = storage.list_databases().unwrap();
    assert!(dbs.is_empty());
}

#[test]
fn test_storage_opens_database() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let db = storage.open_database("mydb");
    assert!(db.is_ok());
}

#[test]
fn test_database_metadata() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let db = storage.open_database("mydb").unwrap();
    let meta = db.metadata();
    assert_eq!(meta.name.to_string(), "mydb");
    assert_eq!(meta.version, 0);
    assert!(meta.stores.is_empty());
}

#[test]
fn test_database_capabilities() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let db = storage.open_database("mydb").unwrap();
    let caps = db.capabilities();
    assert!(caps.snapshot_isolation);
    assert!(!caps.durable);
    assert!(caps.concurrent);
}

#[test]
fn test_begin_transaction() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();
    let txn = db.begin(TxnMode::ReadWrite, &[1], Durability::Default);
    assert!(txn.is_ok());
}

#[test]
fn test_txn_put_and_get() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    // Begin request savepoint
    txn.begin_request().unwrap();

    // Put a record
    txn.put(1, b"key1", b"value1", false).unwrap();

    // Get the record
    let val = txn.get(1, b"key1").unwrap();
    assert_eq!(val, Some(b"value1".to_vec()));

    // Commit request
    txn.commit_request().unwrap();

    // Commit transaction
    txn.commit().unwrap();
}

#[test]
fn test_txn_delete_record() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"value1", false).unwrap();
    txn.commit_request().unwrap();

    txn.begin_request().unwrap();
    let deleted = txn.delete_record(1, b"key1").unwrap();
    assert!(deleted);
    txn.commit_request().unwrap();

    // Record should be gone
    txn.begin_request().unwrap();
    let val = txn.get(1, b"key1").unwrap();
    assert!(val.is_none());
    txn.commit_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_txn_savepoint_rollback() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    // Put initial value
    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"initial", false).unwrap();
    txn.commit_request().unwrap();

    // Start a new request and modify
    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"modified", false).unwrap();

    // Rollback the request
    txn.rollback_request().unwrap();

    // Value should be restored
    txn.begin_request().unwrap();
    let val = txn.get(1, b"key1").unwrap();
    assert_eq!(val, Some(b"initial".to_vec()));
    txn.commit_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_txn_key_generator() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();

    // Set key generator
    txn.key_gen_set(1, 100.0).unwrap();

    // Read it back
    let current = txn.key_gen_current(1).unwrap();
    assert_eq!(current, 100.0);

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn test_txn_key_generator_savepoint_rollback() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    // Set initial value
    txn.begin_request().unwrap();
    txn.key_gen_set(1, 100.0).unwrap();
    txn.commit_request().unwrap();

    // Modify and rollback
    txn.begin_request().unwrap();
    txn.key_gen_set(1, 200.0).unwrap();
    txn.rollback_request().unwrap();

    // Should be restored
    txn.begin_request().unwrap();
    let current = txn.key_gen_current(1).unwrap();
    assert_eq!(current, 100.0);
    txn.commit_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_txn_index_operations() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();

    // Put index entry
    txn.index_put(1, b"idx_key1", b"pk1", false).unwrap();

    // Delete index entry
    txn.index_delete(1, b"idx_key1", b"pk1").unwrap();

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn test_txn_scope_check() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    // Transaction only covers store 1
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();

    // Should fail for store 2 (not in scope)
    let result = txn.put(2, b"key1", b"value1", false);
    assert!(result.is_err());

    txn.rollback_request().unwrap();
    txn.abort().unwrap();
}

#[test]
fn test_txn_readonly_check() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();

    // Should fail for write operations
    let result = txn.put(1, b"key1", b"value1", false);
    assert!(result.is_err());

    txn.rollback_request().unwrap();
    txn.abort().unwrap();
}

#[test]
fn test_storage_delete_database() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();

    // Create a database
    let _db = storage.open_database("mydb").unwrap();

    // Delete it
    storage.delete_database("mydb").unwrap();

    // List should be empty
    let dbs = storage.list_databases().unwrap();
    assert!(dbs.is_empty());
}

#[test]
fn test_storage_usage_bytes() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();

    let usage = storage.usage_bytes().unwrap();
    assert_eq!(usage, 0);

    // Create a database
    let _db = storage.open_database("mydb").unwrap();

    let usage = storage.usage_bytes().unwrap();
    assert!(usage > 0);
}
