//! Concurrency tests: multiple readers during active writer.

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, SourceRef, StorageKey, TxnMode};
use boa_idb_sqlite::SqliteBackendFactory;
use std::sync::Arc;
use std::thread;

#[test]
fn test_concurrent_readers_during_write() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = Arc::new(SqliteBackendFactory::new(tmp.path()));

    // Setup: create database with store and initial data
    {
        let storage = factory.open_storage(&StorageKey::new("test")).unwrap();
        let mut db = storage.open_database("concurrent_db").unwrap();

        let store_ids = db
            .metadata()
            .stores
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>();
        let mut txn = db
            .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
            .unwrap();
        txn.set_version(1).unwrap();
        let store_id = txn
            .create_store(&StoreSpec {
                name: Utf16String::from("items"),
                key_path: KeyPath::Empty,
                auto_increment: false,
            })
            .unwrap();
        txn.commit().unwrap();

        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
            .unwrap();
        txn.put(store_id, b"initial", b"data", false).unwrap();
        txn.commit().unwrap();
    }

    // Hold a writer transaction OPEN (uncommitted write) while readers run:
    // readers must proceed without blocking and must not see uncommitted data.
    let storage = factory.open_storage(&StorageKey::new("test")).unwrap();
    let mut writer_db = storage.open_database("concurrent_db").unwrap();
    let store_id = writer_db.metadata().stores[0].id;
    let mut writer_txn = writer_db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    writer_txn
        .put(store_id, b"uncommitted", b"invisible", false)
        .unwrap();

    // Spawn 10 reader threads
    let mut handles = Vec::new();
    for i in 0..10 {
        let factory_clone = factory.clone();
        handles.push(thread::spawn(move || {
            let storage = factory_clone
                .open_storage(&StorageKey::new("test"))
                .unwrap();
            let mut db = storage.open_database("concurrent_db").unwrap();
            let store_id = db.metadata().stores[0].id;

            // Each reader reads committed data (never blocks on the writer).
            let mut txn = db
                .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
                .unwrap();
            let range = EncodedRange::all();
            let count = txn.count(SourceRef::Store(store_id), &range).unwrap();
            assert!(count >= 1, "Reader {i} should see at least 1 record");
            let val = txn.get(store_id, b"initial").unwrap();
            assert_eq!(val, Some(b"data".to_vec()));
            // Uncommitted writer data stays invisible.
            assert_eq!(txn.get(store_id, b"uncommitted").unwrap(), None);
            txn.commit().unwrap();
        }));
    }

    // Wait for all readers (a writer-reader deadlock would hang here).
    for handle in handles {
        handle.join().unwrap();
    }

    // Now commit the writer; a fresh reader sees the new data.
    writer_txn.commit().unwrap();
    let mut db = storage.open_database("concurrent_db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(
        txn.get(store_id, b"uncommitted").unwrap(),
        Some(b"invisible".to_vec())
    );
    txn.commit().unwrap();
}

#[test]
fn test_readers_see_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = Arc::new(SqliteBackendFactory::new(tmp.path()));

    // Setup
    {
        let storage = factory.open_storage(&StorageKey::new("test")).unwrap();
        let mut db = storage.open_database("snap_db").unwrap();

        let store_ids = db
            .metadata()
            .stores
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>();
        let mut txn = db
            .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
            .unwrap();
        txn.set_version(1).unwrap();
        let store_id = txn
            .create_store(&StoreSpec {
                name: Utf16String::from("items"),
                key_path: KeyPath::Empty,
                auto_increment: false,
            })
            .unwrap();
        txn.commit().unwrap();

        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
            .unwrap();
        txn.put(store_id, b"v1", b"old", false).unwrap();
        txn.commit().unwrap();
    }

    // Start a reader that holds a snapshot
    let factory_clone = factory.clone();
    let reader_handle = thread::spawn(move || {
        let storage = factory_clone
            .open_storage(&StorageKey::new("test"))
            .unwrap();
        let mut db = storage.open_database("snap_db").unwrap();
        let store_id = db.metadata().stores[0].id;

        // Start a long-lived read transaction
        let mut txn = db
            .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
            .unwrap();

        // Read initial value
        let val = txn.get(store_id, b"v1").unwrap();
        assert_eq!(val, Some(b"old".to_vec()));

        // Signal that we've read (give writer time to write)
        thread::sleep(std::time::Duration::from_millis(50));

        // Should still see old value (snapshot isolation)
        let val = txn.get(store_id, b"v1").unwrap();
        assert_eq!(val, Some(b"old".to_vec()));

        // Should not see the new key
        let val = txn.get(store_id, b"v2").unwrap();
        assert_eq!(val, None);
    });

    // Writer modifies data while reader holds snapshot
    {
        let storage = factory.open_storage(&StorageKey::new("test")).unwrap();
        let mut db = storage.open_database("snap_db").unwrap();
        let store_id = db.metadata().stores[0].id;

        thread::sleep(std::time::Duration::from_millis(10));

        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
            .unwrap();
        txn.put(store_id, b"v1", b"new", false).unwrap();
        txn.put(store_id, b"v2", b"added", false).unwrap();
        txn.commit().unwrap();
    }

    reader_handle.join().unwrap();
}
