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
    // No snapshot isolation (M7-B H-MEM): lazy cursors re-read committed
    // state per step for O(1) memory instead of snapshotting the range.
    // The L1 driver never branches on this flag.
    assert!(!caps.snapshot_isolation);
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

    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"value1", false).unwrap();

    let val = txn.get(1, b"key1").unwrap();
    assert_eq!(val, Some(b"value1".to_vec()));

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn test_txn_get_committed_data() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    // First transaction: write data
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[1], Durability::Default)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(1, b"key1", b"value1", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }

    // Second transaction: read committed data
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[1], Durability::Default)
            .unwrap();
        txn.begin_request().unwrap();
        let val = txn.get(1, b"key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
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

    txn.begin_request().unwrap();
    let val = txn.get(1, b"key1").unwrap();
    assert!(val.is_none());
    txn.commit_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_txn_delete_nonexistent() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();
    let deleted = txn.delete_record(1, b"nonexistent").unwrap();
    assert!(!deleted);
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

    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"initial", false).unwrap();
    txn.commit_request().unwrap();

    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"modified", false).unwrap();
    txn.rollback_request().unwrap();

    txn.begin_request().unwrap();
    let val = txn.get(1, b"key1").unwrap();
    assert_eq!(val, Some(b"initial".to_vec()));
    txn.commit_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_txn_commit_request_discards_undo() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    // First request: write
    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"value1", false).unwrap();
    txn.commit_request().unwrap();

    // Second request: modify and commit
    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"value2", false).unwrap();
    txn.commit_request().unwrap();

    // The value should be value2 (committed), not value1
    txn.begin_request().unwrap();
    let val = txn.get(1, b"key1").unwrap();
    assert_eq!(val, Some(b"value2".to_vec()));
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
    txn.key_gen_set(1, 100.0).unwrap();
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

    txn.begin_request().unwrap();
    txn.key_gen_set(1, 100.0).unwrap();
    txn.commit_request().unwrap();

    txn.begin_request().unwrap();
    txn.key_gen_set(1, 200.0).unwrap();
    txn.rollback_request().unwrap();

    txn.begin_request().unwrap();
    let current = txn.key_gen_current(1).unwrap();
    assert_eq!(current, 100.0);
    txn.commit_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_txn_key_generator_persists_across_transactions() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    // First transaction: set key generator
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[1], Durability::Default)
            .unwrap();
        txn.begin_request().unwrap();
        txn.key_gen_set(1, 42.0).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }

    // Second transaction: read key generator
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[1], Durability::Default)
            .unwrap();
        txn.begin_request().unwrap();
        let current = txn.key_gen_current(1).unwrap();
        assert_eq!(current, 42.0);
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
}

#[test]
fn test_txn_index_unique_constraint() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();

    // First entry should succeed
    txn.index_put(1, b"idx_key1", b"pk1", true).unwrap();

    // Second entry with same idx_key but different pk should fail
    let result = txn.index_put(1, b"idx_key1", b"pk2", true);
    assert!(result.is_err());

    txn.rollback_request().unwrap();
    txn.abort().unwrap();
}

#[test]
fn test_txn_index_non_unique_allows_duplicates() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();

    // Both should succeed with non-unique index
    txn.index_put(1, b"idx_key1", b"pk1", false).unwrap();
    txn.index_put(1, b"idx_key1", b"pk2", false).unwrap();

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn test_txn_index_delete_by_primary() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();

    // Add entries to index 1
    txn.index_put(1, b"idx_a", b"pk1", false).unwrap();
    txn.index_put(1, b"idx_b", b"pk1", false).unwrap();

    // Add entry to index 2 with same primary key
    txn.index_put(2, b"idx_c", b"pk1", false).unwrap();

    // Delete by primary from index 1 only
    txn.index_delete_by_primary(1, b"pk1").unwrap();

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn test_txn_scope_check() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();
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
    let result = txn.put(1, b"key1", b"value1", false);
    assert!(result.is_err());
    txn.rollback_request().unwrap();

    txn.abort().unwrap();
}

#[test]
fn test_txn_no_overwrite() {
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
    let result = txn.put(1, b"key1", b"value2", true);
    assert!(result.is_err());
    txn.rollback_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_txn_clear() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"value1", false).unwrap();
    txn.put(1, b"key2", b"value2", false).unwrap();
    txn.commit_request().unwrap();

    txn.begin_request().unwrap();
    txn.clear(1).unwrap();
    txn.commit_request().unwrap();

    txn.begin_request().unwrap();
    assert!(txn.get(1, b"key1").unwrap().is_none());
    assert!(txn.get(1, b"key2").unwrap().is_none());
    txn.commit_request().unwrap();

    txn.commit().unwrap();
}

#[test]
fn test_storage_delete_database() {
    let factory = MemoryBackendFactory::new();
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage = factory.open_storage(&key).unwrap();

    let _db = storage.open_database("mydb").unwrap();
    storage.delete_database("mydb").unwrap();

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

    let mut db = storage.open_database("mydb").unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(1, b"key1", b"value1", false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();

    let usage = storage.usage_bytes().unwrap();
    assert!(usage > 0);
}

#[test]
fn test_scheduler_versionchange_blocks_all() {
    use boa_idb_core::engine::scheduler::{TransactionScheduler, TxnQueueItem};

    let mut sched = TransactionScheduler::new();

    // VersionChange is pending
    sched.enqueue(TxnQueueItem {
        id: 1,
        mode: TxnMode::VersionChange,
        scope: vec![1],
    });

    // ReadOnly to different store should be blocked
    sched.enqueue(TxnQueueItem {
        id: 2,
        mode: TxnMode::ReadOnly,
        scope: vec![2],
    });

    let ready = sched.poll_ready();
    assert_eq!(ready, vec![1]);

    // Now pending VersionChange should block new transactions
    sched.enqueue(TxnQueueItem {
        id: 3,
        mode: TxnMode::ReadOnly,
        scope: vec![2],
    });

    let ready = sched.poll_ready();
    assert!(ready.is_empty());
}
