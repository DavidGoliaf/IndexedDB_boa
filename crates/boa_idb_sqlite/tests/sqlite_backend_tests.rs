//! Full CRUD cycle tests for the `SQLite` backend.

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::{CursorSeek, IndexSpec, StoreSpec};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Direction, Durability, SourceRef, TxnMode};
use boa_idb_sqlite::SqliteBackendFactory;

fn test_factory() -> (SqliteBackendFactory, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    (factory, tmp)
}

#[test]
fn test_create_store_and_put_get() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

    // Create store via VersionChange
    {
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

        // Put and get
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store_id, b"key1", b"value1", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();

        // Read back
        let mut txn = db
            .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
            .unwrap();
        let val = txn.get(store_id, b"key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
    }
}

#[test]
fn test_put_overwrite() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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

    // Put
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"key1", b"v1", false).unwrap();
    txn.put(store_id, b"key1", b"v2", false).unwrap(); // overwrite
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let val = txn.get(store_id, b"key1").unwrap();
    assert_eq!(val, Some(b"v2".to_vec()));
}

#[test]
fn test_put_no_overwrite_constraint() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    txn.put(store_id, b"key1", b"v1", false).unwrap();
    let result = txn.put(store_id, b"key1", b"v2", true); // no_overwrite
    assert!(result.is_err());
}

#[test]
fn test_delete_record() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    txn.put(store_id, b"key1", b"v1", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    let deleted = txn.delete_record(store_id, b"key1").unwrap();
    assert!(deleted);
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let val = txn.get(store_id, b"key1").unwrap();
    assert_eq!(val, None);
}

#[test]
fn test_clear_store() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    txn.put(store_id, b"k1", b"v1", false).unwrap();
    txn.put(store_id, b"k2", b"v2", false).unwrap();
    txn.put(store_id, b"k3", b"v3", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.clear(store_id).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::all();
    let count = txn.count(SourceRef::Store(store_id), &range).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_count_range() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    for i in 0..10 {
        let key = format!("key{i:02}");
        let val = format!("val{i}");
        txn.put(store_id, key.as_bytes(), val.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::all();
    let count = txn.count(SourceRef::Store(store_id), &range).unwrap();
    assert_eq!(count, 10);
}

#[test]
fn test_scan_forward() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    txn.put(store_id, b"a", b"1", false).unwrap();
    txn.put(store_id, b"b", b"2", false).unwrap();
    txn.put(store_id, b"c", b"3", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::all();
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &range, Direction::Next, false)
        .unwrap();

    assert!(
        cursor
            .seek(boa_idb_core::backend::types::CursorSeek::First)
            .unwrap()
    );
    assert_eq!(cursor.current_key(), b"a");
    assert_eq!(cursor.current_value(), Some(b"1".as_ref()));

    assert!(cursor.step(1).unwrap());
    assert_eq!(cursor.current_key(), b"b");
    assert_eq!(cursor.current_value(), Some(b"2".as_ref()));

    assert!(cursor.step(1).unwrap());
    assert_eq!(cursor.current_key(), b"c");
    assert_eq!(cursor.current_value(), Some(b"3".as_ref()));

    assert!(!cursor.step(1).unwrap());
}

#[test]
fn test_scan_backward() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    txn.put(store_id, b"a", b"1", false).unwrap();
    txn.put(store_id, b"b", b"2", false).unwrap();
    txn.put(store_id, b"c", b"3", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::all();
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &range, Direction::Prev, false)
        .unwrap();

    assert!(
        cursor
            .seek(boa_idb_core::backend::types::CursorSeek::First)
            .unwrap()
    );
    assert_eq!(cursor.current_key(), b"c");

    assert!(cursor.step(1).unwrap());
    assert_eq!(cursor.current_key(), b"b");

    assert!(cursor.step(1).unwrap());
    assert_eq!(cursor.current_key(), b"a");

    assert!(!cursor.step(1).unwrap());
}

#[test]
fn test_savepoint_rollback() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    txn.put(store_id, b"key1", b"v1", false).unwrap();

    // Begin a request (savepoint), put, then rollback
    txn.begin_request().unwrap();
    txn.put(store_id, b"key2", b"v2", false).unwrap();
    txn.rollback_request().unwrap();

    txn.commit().unwrap();

    // key1 should exist, key2 should not
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"key1").unwrap(), Some(b"v1".to_vec()));
    assert_eq!(txn.get(store_id, b"key2").unwrap(), None);
}

#[test]
fn test_persistence_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());

    // Create and write
    {
        let storage = factory
            .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
            .unwrap();
        let mut db = storage.open_database("persist_db").unwrap();

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
        txn.put(store_id, b"persistent", b"data", false).unwrap();
        txn.commit().unwrap();
    }

    // Reopen and read
    {
        let factory2 = SqliteBackendFactory::new(tmp.path());
        let storage = factory2
            .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
            .unwrap();
        let mut db = storage.open_database("persist_db").unwrap();

        assert_eq!(db.metadata().version, 1);
        assert_eq!(db.metadata().stores.len(), 1);
        let store_id = db.metadata().stores[0].id;

        let mut txn = db
            .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
            .unwrap();
        let val = txn.get(store_id, b"persistent").unwrap();
        assert_eq!(val, Some(b"data".to_vec()));
    }
}

#[test]
fn test_index_operations() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_name"),
                key_path: KeyPath::parse_single("name").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    // Put records and index entries
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"1", b"alice", false).unwrap();
    txn.index_put(index_id, b"alice", b"1", false).unwrap();
    txn.put(store_id, b"2", b"bob", false).unwrap();
    txn.index_put(index_id, b"bob", b"2", false).unwrap();
    txn.commit().unwrap();

    // Count index entries
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::all();
    let count = txn
        .count(
            SourceRef::Index {
                store: store_id,
                index: index_id,
            },
            &range,
        )
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn test_key_gen() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("testdb").unwrap();

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
            auto_increment: true,
        })
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    let key_gen_val = txn.key_gen_current(store_id).unwrap();
    assert!((key_gen_val - 1.0).abs() < f64::EPSILON);

    txn.key_gen_set(store_id, 42.5).unwrap();
    let key_gen_val = txn.key_gen_current(store_id).unwrap();
    assert!((key_gen_val - 42.5).abs() < f64::EPSILON);
    txn.commit().unwrap();
}

#[test]
fn test_list_databases() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();

    storage.open_database("db1").unwrap();
    storage.open_database("db2").unwrap();

    let dbs = storage.list_databases().unwrap();
    assert_eq!(dbs.len(), 2);
    let names: Vec<&str> = dbs.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"db1"));
    assert!(names.contains(&"db2"));
}

#[test]
fn test_delete_database() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();

    storage.open_database("to_delete").unwrap();
    assert_eq!(storage.list_databases().unwrap().len(), 1);

    storage.delete_database("to_delete").unwrap();
    assert_eq!(storage.list_databases().unwrap().len(), 0);
}

#[test]
fn test_factory_root_and_storage_key_hashing() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    assert_eq!(factory.root(), tmp.path());

    // Storage keys are hashed into safe directory names (R8.1.4): the key text
    // never appears verbatim in the filesystem path.
    let key = boa_idb_core::proto::StorageKey::new("https://example.com/profile");
    let storage = factory.open_storage(&key).unwrap();
    let dir = factory.storage_dir(&key);
    assert_eq!(
        dir,
        tmp.path().join(boa_idb_sqlite::storage_dir_name(
            "https://example.com/profile"
        ))
    );
    assert!(dir.join("registry.sqlite").exists());
    assert!(!dir.to_string_lossy().contains("example"));

    // Distinct keys map to distinct directories.
    let other = boa_idb_core::proto::StorageKey::new("https://example.com/other");
    assert_ne!(factory.storage_dir(&key), factory.storage_dir(&other));

    // Empty and reopen round-trips.
    assert!(storage.list_databases().unwrap().is_empty());
}

#[test]
fn test_delete_database_removes_blob_dir() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("with_blobs").unwrap();

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
    let big = vec![0x44u8; 512 * 1024];
    txn.put(store_id, b"big", &big, false).unwrap();
    txn.commit().unwrap();

    drop(db);

    // Blob directory exists while the database exists
    let blob_dir = tmp_dir_blobs(&factory);
    assert!(blob_dir.exists());
    assert!(count_bin_files(&blob_dir) > 0, "blob files exist on disk");

    // Usage accounting includes the blob files.
    let usage_with_blobs = storage.usage_bytes().unwrap();
    drop(storage);

    // Reopen only for deletion path coverage.
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    assert!(!storage.list_databases().unwrap().is_empty());

    storage.delete_database("with_blobs").unwrap();
    assert_eq!(
        count_bin_files(&blob_dir),
        0,
        "blob files must be removed with the database"
    );
    assert!(
        storage.usage_bytes().unwrap() < usage_with_blobs,
        "usage shrinks after deletion"
    );
}

#[test]
fn test_delete_missing_database_is_noop() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();

    assert_eq!(storage.list_databases().unwrap().len(), 0);
    storage.delete_database("never_created").unwrap();
    assert_eq!(storage.list_databases().unwrap().len(), 0);
}

#[test]
fn test_storage_root_accessor() {
    use boa_idb_sqlite::SqliteStorage;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(boa_idb_sqlite::storage_dir_name("test"));
    let storage = SqliteStorage::open(&dir, &boa_idb_core::proto::StorageKey::new("test")).unwrap();
    assert_eq!(storage.root(), dir);
}

#[test]
fn test_corrupt_key_paths_are_rejected() {
    use rusqlite::Connection;

    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();

    // Create a database with a store that has a key path.
    let mut db = storage.open_database("corrupt_db").unwrap();
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
    txn.create_store(&StoreSpec {
        name: Utf16String::from("people"),
        key_path: KeyPath::parse_single("id").unwrap(),
        auto_increment: false,
    })
    .unwrap();
    txn.commit().unwrap();
    drop(db);

    let db_file = db_file_path(&factory);
    // Invalid/malformed key path blobs must surface as Corrupted metadata.
    let corrupt_payloads: Vec<Vec<u8>> = vec![
        vec![0x07],                               // unknown tag
        vec![0x02, 0x01],                         // array, truncated length prefix
        vec![0x02, 0x08, 0x00, 0x00, 0x00, 0x61], // array, truncated payload
        vec![0x01, 0x61],                         // single path, odd UTF-16 length
        vec![
            0x02, 0x08, 0x00, 0x00, 0x00, 0x31, 0x00, 0x61, 0x00, 0x62, 0x00, 0x63, 0x00,
        ], // "1abc" invalid identifier
    ];

    for payload in corrupt_payloads {
        let conn = Connection::open(&db_file).unwrap();
        let changed = conn
            .execute(
                "UPDATE object_stores SET key_path = ?1 WHERE id = 1",
                [&payload],
            )
            .unwrap();
        assert_eq!(changed, 1, "store row must exist for corruption");
        drop(conn);

        let err = storage
            .open_database("corrupt_db")
            .err()
            .expect("corrupt database must fail to open");
        assert!(
            matches!(
                err,
                boa_idb_core::backend::error::BackendError::Corrupted(_)
            ),
            "malformed key path {payload:?} must yield Corrupted, got {err:?}"
        );
    }
}

/// Returns the `db-*.sqlite` path for the single database in storage "test".
fn db_file_path(factory: &SqliteBackendFactory) -> std::path::PathBuf {
    let key = boa_idb_core::proto::StorageKey::new("test");
    let mut files: Vec<_> = std::fs::read_dir(factory.storage_dir(&key))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let fname = e.file_name();
            let name = fname.to_string_lossy();
            // Exclude WAL/SHM sidecars (`db-….sqlite-wal`); a live pooled
            // connection keeps those files around.
            name.starts_with("db-") && name.ends_with(".sqlite")
        })
        .map(|e| e.path())
        .collect();
    assert_eq!(files.len(), 1);
    files.pop().unwrap()
}

/// Returns the blob root directory for the storage key "test".
fn tmp_dir_blobs(factory: &SqliteBackendFactory) -> std::path::PathBuf {
    let key = boa_idb_core::proto::StorageKey::new("test");
    factory.storage_dir(&key).join("blobs")
}

/// Recursively counts `.bin` files under `dir`.
fn count_bin_files(dir: &std::path::Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                count += count_bin_files(&path);
            } else if path.extension().is_some_and(|e| e == "bin") {
                count += 1;
            }
        }
    }
    count
}

/// Creates a database with a single "items" store and returns it with the store id.
fn open_items_db(
    factory: &SqliteBackendFactory,
    name: &str,
) -> (Box<dyn boa_idb_core::backend::traits::Database>, u64) {
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database(name).unwrap();
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
    (db, store_id)
}

/// Inserts `k01`..=`k10` into the store.
fn put_range(db: &mut Box<dyn boa_idb_core::backend::traits::Database>, store_id: u64) {
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for i in 1..=10 {
        let key = format!("k{i:02}");
        let val = format!("v{i:02}");
        txn.put(store_id, key.as_bytes(), val.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();
}

#[test]
fn test_store_cursor_bounded_range() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "cursor_range_db");
    put_range(&mut db, store_id);

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::bound(b"k03".to_vec(), false, b"k06".to_vec(), false);
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &range, Direction::Next, false)
        .unwrap();

    let mut keys = Vec::new();
    if cursor.seek(CursorSeek::First).unwrap() {
        loop {
            keys.push(cursor.current_key().to_vec());
            if !cursor.step(1).unwrap() {
                break;
            }
        }
    }
    drop(cursor);

    assert_eq!(
        keys,
        vec![
            b"k03".to_vec(),
            b"k04".to_vec(),
            b"k05".to_vec(),
            b"k06".to_vec()
        ]
    );
}

#[test]
fn test_store_cursor_open_bounds() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "cursor_open_db");
    put_range(&mut db, store_id);

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::bound(b"k03".to_vec(), true, b"k06".to_vec(), true);
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &range, Direction::Next, false)
        .unwrap();

    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.current_key(), b"k04");
    assert_eq!(cursor.current_value(), Some(b"v04".as_ref()));
    assert!(cursor.step(1).unwrap());
    assert_eq!(cursor.current_key(), b"k05");
    drop(cursor);

    // Bounded single-key ranges
    let single = EncodedRange::only(b"k05".to_vec());
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &single, Direction::Next, false)
        .unwrap();
    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.current_key(), b"k05");
    assert!(!cursor.step(1).unwrap());
    drop(cursor);

    // Exclusive range that matches nothing
    let empty = EncodedRange::bound(b"k07".to_vec(), true, b"k07".to_vec(), false);
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &empty, Direction::Next, false)
        .unwrap();
    assert!(!cursor.seek(CursorSeek::First).unwrap());
}

#[test]
fn test_store_cursor_key_only() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "cursor_keyonly_db");
    put_range(&mut db, store_id);

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::all();
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &range, Direction::Next, true)
        .unwrap();
    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.current_key(), b"k01");
    assert_eq!(cursor.current_value(), None);
}

#[test]
fn test_store_cursor_seek_and_step() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "cursor_seek_db");
    put_range(&mut db, store_id);

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::all();
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &range, Direction::Next, false)
        .unwrap();

    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.current_key(), b"k01");

    // Multi-step
    assert!(cursor.step(3).unwrap());
    assert_eq!(cursor.current_key(), b"k04");

    // Seek to a specific key (inclusive)
    assert!(cursor.seek(CursorSeek::Key(b"k07".to_vec())).unwrap());
    assert_eq!(cursor.current_key(), b"k07");

    // Seek to a key past the end -> exhausted
    assert!(!cursor.seek(CursorSeek::Key(b"zzz".to_vec())).unwrap());
    drop(cursor);

    // Seek by key + primary key (store cursors use the record key as pkey)
    let mut cursor = txn
        .scan(SourceRef::Store(store_id), &range, Direction::Next, false)
        .unwrap();
    assert!(
        cursor
            .seek(CursorSeek::KeyAndPrimaryKey {
                key: b"k02".to_vec(),
                pkey: b"k02".to_vec(),
            })
            .unwrap()
    );
    assert_eq!(cursor.current_key(), b"k02");
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_index_cursor_next_and_prev() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("index_cursor_db").unwrap();

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
            name: Utf16String::from("people"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_group"),
                key_path: KeyPath::parse_single("group").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for (pk, group) in [
        ("1", "g"),
        ("2", "r"),
        ("3", "r"),
        ("4", "r"),
        ("5", "g"),
        ("6", "t"),
    ] {
        txn.put(store_id, pk.as_bytes(), format!("v{pk}").as_bytes(), false)
            .unwrap();
        txn.index_put(index_id, group.as_bytes(), pk.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    // Next: (key, pkey) ascending
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::Next, false)
        .unwrap();
    let mut rows = Vec::new();
    if cursor.seek(CursorSeek::First).unwrap() {
        loop {
            rows.push((
                cursor.current_key().to_vec(),
                cursor.current_primary_key().to_vec(),
                cursor.current_value().map(<[u8]>::to_vec),
            ));
            if !cursor.step(1).unwrap() {
                break;
            }
        }
    }
    assert_eq!(
        rows,
        vec![
            (b"g".to_vec(), b"1".to_vec(), Some(b"v1".to_vec())),
            (b"g".to_vec(), b"5".to_vec(), Some(b"v5".to_vec())),
            (b"r".to_vec(), b"2".to_vec(), Some(b"v2".to_vec())),
            (b"r".to_vec(), b"3".to_vec(), Some(b"v3".to_vec())),
            (b"r".to_vec(), b"4".to_vec(), Some(b"v4".to_vec())),
            (b"t".to_vec(), b"6".to_vec(), Some(b"v6".to_vec())),
        ]
    );
    drop(cursor);

    // Prev: (key, pkey) descending
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::Prev, false)
        .unwrap();
    let mut rows = Vec::new();
    if cursor.seek(CursorSeek::First).unwrap() {
        loop {
            rows.push((
                cursor.current_key().to_vec(),
                cursor.current_primary_key().to_vec(),
            ));
            if !cursor.step(1).unwrap() {
                break;
            }
        }
    }
    assert_eq!(
        rows,
        vec![
            (b"t".to_vec(), b"6".to_vec()),
            (b"r".to_vec(), b"4".to_vec()),
            (b"r".to_vec(), b"3".to_vec()),
            (b"r".to_vec(), b"2".to_vec()),
            (b"g".to_vec(), b"5".to_vec()),
            (b"g".to_vec(), b"1".to_vec()),
        ]
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn test_index_cursor_unique_directions() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("index_unique_db").unwrap();

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
            name: Utf16String::from("people"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_group"),
                key_path: KeyPath::parse_single("group").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for (pk, group) in [
        ("1", "g"),
        ("2", "r"),
        ("3", "r"),
        ("4", "r"),
        ("5", "g"),
        ("6", "t"),
    ] {
        txn.put(store_id, pk.as_bytes(), format!("v{pk}").as_bytes(), false)
            .unwrap();
        txn.index_put(index_id, group.as_bytes(), pk.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    // NextUnique: one entry per index key, smallest primary key, keys ascending
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::NextUnique, false)
        .unwrap();
    let mut rows = Vec::new();
    if cursor.seek(CursorSeek::First).unwrap() {
        loop {
            rows.push((
                cursor.current_key().to_vec(),
                cursor.current_primary_key().to_vec(),
                cursor.current_value().map(<[u8]>::to_vec),
            ));
            if !cursor.step(1).unwrap() {
                break;
            }
        }
    }
    assert_eq!(
        rows,
        vec![
            (b"g".to_vec(), b"1".to_vec(), Some(b"v1".to_vec())),
            (b"r".to_vec(), b"2".to_vec(), Some(b"v2".to_vec())),
            (b"t".to_vec(), b"6".to_vec(), Some(b"v6".to_vec())),
        ]
    );
    drop(cursor);

    // PrevUnique: smallest primary key per key, keys descending
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::PrevUnique, false)
        .unwrap();
    let mut rows = Vec::new();
    if cursor.seek(CursorSeek::First).unwrap() {
        loop {
            rows.push((
                cursor.current_key().to_vec(),
                cursor.current_primary_key().to_vec(),
            ));
            if !cursor.step(1).unwrap() {
                break;
            }
        }
    }
    assert_eq!(
        rows,
        vec![
            (b"t".to_vec(), b"6".to_vec()),
            (b"r".to_vec(), b"2".to_vec()),
            (b"g".to_vec(), b"1".to_vec()),
        ]
    );
}

/// Regression: seeks must honor the cursor direction. A `Prev` cursor seeked
/// to `b` must land on `b` (or the next-lower key), never on `c`.
#[test]
fn test_store_cursor_prev_seek_is_direction_aware() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "cursor_prev_seek_db");
    put_range(&mut db, store_id);

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let mut cursor = txn
        .scan(
            SourceRef::Store(store_id),
            &EncodedRange::all(),
            Direction::Prev,
            false,
        )
        .unwrap();

    // First in Prev order is the largest key.
    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.current_key(), b"k10");

    // Seek lands on the target itself...
    assert!(cursor.seek(CursorSeek::Key(b"k05".to_vec())).unwrap());
    assert_eq!(cursor.current_key(), b"k05");
    // ...and stepping continues downward.
    assert!(cursor.step(1).unwrap());
    assert_eq!(cursor.current_key(), b"k04");

    // Seek to a missing key positions at the next-lower existing key.
    assert!(cursor.seek(CursorSeek::Key(b"k055".to_vec())).unwrap());
    assert_eq!(cursor.current_key(), b"k05");

    // Seek with key + primary key is direction-aware too.
    assert!(
        cursor
            .seek(CursorSeek::KeyAndPrimaryKey {
                key: b"k03".to_vec(),
                pkey: b"k03".to_vec(),
            })
            .unwrap()
    );
    assert_eq!(cursor.current_key(), b"k03");

    // Below the lowest key there is nothing left.
    assert!(!cursor.seek(CursorSeek::Key(b"k00".to_vec())).unwrap());
}

/// Regression: index seeks must honor direction *and* the (key, pkey) tuple.
#[test]
fn test_index_cursor_seek_key_and_pkey_both_directions() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("index_seek_db").unwrap();

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
            name: Utf16String::from("people"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_group"),
                key_path: KeyPath::parse_single("group").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for (pk, group) in [("1", "g"), ("2", "r"), ("3", "r"), ("4", "r"), ("5", "g")] {
        txn.put(store_id, pk.as_bytes(), format!("v{pk}").as_bytes(), false)
            .unwrap();
        txn.index_put(index_id, group.as_bytes(), pk.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();

    // Next: (r, 3) -> step -> (r, 4).
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::Next, false)
        .unwrap();
    assert!(
        cursor
            .seek(CursorSeek::KeyAndPrimaryKey {
                key: b"r".to_vec(),
                pkey: b"3".to_vec(),
            })
            .unwrap()
    );
    assert_eq!(
        (cursor.current_key(), cursor.current_primary_key()),
        (b"r".as_ref(), b"3".as_ref())
    );
    assert!(cursor.step(1).unwrap());
    assert_eq!(
        (cursor.current_key(), cursor.current_primary_key()),
        (b"r".as_ref(), b"4".as_ref())
    );
    drop(cursor);

    // Prev: (r, 3) -> step -> (r, 2) — never the rows above the target.
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::Prev, false)
        .unwrap();
    assert!(
        cursor
            .seek(CursorSeek::KeyAndPrimaryKey {
                key: b"r".to_vec(),
                pkey: b"3".to_vec(),
            })
            .unwrap()
    );
    assert_eq!(
        (cursor.current_key(), cursor.current_primary_key()),
        (b"r".as_ref(), b"3".as_ref())
    );
    assert!(cursor.step(1).unwrap());
    assert_eq!(
        (cursor.current_key(), cursor.current_primary_key()),
        (b"r".as_ref(), b"2".as_ref())
    );
}

/// Regression: unique-cursor seeks position on the representative row of the
/// group (smallest primary key), not on an arbitrary duplicate.
#[test]
fn test_index_cursor_unique_seek() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("index_unique_seek_db").unwrap();

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
            name: Utf16String::from("people"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_group"),
                key_path: KeyPath::parse_single("group").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for (pk, group) in [("1", "g"), ("2", "r"), ("3", "r"), ("4", "r"), ("5", "g")] {
        txn.put(store_id, pk.as_bytes(), format!("v{pk}").as_bytes(), false)
            .unwrap();
        txn.index_put(index_id, group.as_bytes(), pk.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();

    // NextUnique seek to "r" lands on the group's smallest primary key.
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::NextUnique, false)
        .unwrap();
    assert!(cursor.seek(CursorSeek::Key(b"r".to_vec())).unwrap());
    assert_eq!(cursor.current_key(), b"r");
    assert_eq!(cursor.current_primary_key(), b"2");
    drop(cursor);

    // PrevUnique seek to "r" also lands on the group's smallest primary key.
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::PrevUnique, false)
        .unwrap();
    assert!(cursor.seek(CursorSeek::Key(b"r".to_vec())).unwrap());
    assert_eq!(cursor.current_key(), b"r");
    assert_eq!(cursor.current_primary_key(), b"2");
}

#[test]
fn test_index_cursor_bounded_and_key_only() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("index_bound_db").unwrap();

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
            name: Utf16String::from("people"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_letter"),
                key_path: KeyPath::parse_single("letter").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for (pk, letter) in [("1", "a"), ("2", "b"), ("3", "c"), ("4", "d"), ("5", "e")] {
        txn.put(store_id, pk.as_bytes(), format!("v{pk}").as_bytes(), false)
            .unwrap();
        txn.index_put(index_id, letter.as_bytes(), pk.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    // Bounded range over index keys [b, d]
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::bound(b"b".to_vec(), false, b"d".to_vec(), false);
    let mut cursor = txn.scan(src, &range, Direction::Next, false).unwrap();
    let mut keys = Vec::new();
    if cursor.seek(CursorSeek::First).unwrap() {
        loop {
            keys.push(cursor.current_key().to_vec());
            if !cursor.step(1).unwrap() {
                break;
            }
        }
    }
    assert_eq!(keys, vec![b"b".to_vec(), b"c".to_vec(), b"d".to_vec()]);
    drop(cursor);

    // Key-only index cursor returns no values
    let mut cursor = txn
        .scan(src, &EncodedRange::all(), Direction::Next, true)
        .unwrap();
    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.current_key(), b"a");
    assert_eq!(cursor.current_primary_key(), b"1");
    assert_eq!(cursor.current_value(), None);
}

#[test]
fn test_delete_range_bounded() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "delete_range_db");
    put_range(&mut db, store_id);

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::bound(b"k03".to_vec(), false, b"k06".to_vec(), false);
    let deleted = txn.delete_range(store_id, &range).unwrap();
    assert_eq!(deleted, 4);
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"k02").unwrap(), Some(b"v02".to_vec()));
    assert_eq!(txn.get(store_id, b"k03").unwrap(), None);
    assert_eq!(txn.get(store_id, b"k06").unwrap(), None);
    assert_eq!(txn.get(store_id, b"k07").unwrap(), Some(b"v07".to_vec()));
    let count = txn
        .count(SourceRef::Store(store_id), &EncodedRange::all())
        .unwrap();
    assert_eq!(count, 6);
}

#[test]
fn test_schema_store_and_index_ops() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("schema_ops_db").unwrap();

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
    let s1 = txn
        .create_store(&StoreSpec {
            name: Utf16String::from("first"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let s2 = txn
        .create_store(&StoreSpec {
            name: Utf16String::from("second"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let idx = txn
        .create_index(
            s1,
            &IndexSpec {
                name: Utf16String::from("by_name"),
                key_path: KeyPath::parse_single("name").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    // `metadata()` is a snapshot: `flush` synchronizes the view.
    db.flush().unwrap();
    assert_eq!(db.metadata().stores.len(), 2);

    // Rename store and index, then delete an index
    let store_ids = db
        .metadata()
        .stores
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let mut txn = db
        .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
        .unwrap();
    txn.rename_store(s2, "renamed").unwrap();
    txn.rename_index(s1, idx, "renamed_idx").unwrap();
    txn.commit().unwrap();

    db.flush().unwrap();
    let s1_meta = db.metadata().stores.iter().find(|s| s.id == s1).unwrap();
    assert_eq!(s1_meta.indexes.len(), 1);
    assert_eq!(s1_meta.indexes[0].name.to_string(), "renamed_idx");
    let s2_meta = db.metadata().stores.iter().find(|s| s.id == s2).unwrap();
    assert_eq!(s2_meta.name.to_string(), "renamed");

    // Delete index and store
    let store_ids = db
        .metadata()
        .stores
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let mut txn = db
        .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
        .unwrap();
    txn.delete_index(s1, idx).unwrap();
    txn.delete_store(s2).unwrap();
    txn.commit().unwrap();

    // SQLite removes schema rows outright (with FK-cascaded record cleanup);
    // `flush` synchronizes the metadata snapshot.
    db.flush().unwrap();
    let s1_meta = db.metadata().stores.iter().find(|s| s.id == s1).unwrap();
    assert!(s1_meta.indexes.iter().all(|i| i.id != idx));
    assert!(!db.metadata().stores.iter().any(|s| s.id == s2));
}

#[test]
fn test_index_delete_and_by_primary() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("index_del_db").unwrap();

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
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_tag"),
                key_path: KeyPath::parse_single("tag").unwrap(),
                unique: false,
                multi_entry: true,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for (pk, tag) in [("1", "a"), ("1", "b"), ("2", "a")] {
        txn.index_put(index_id, tag.as_bytes(), pk.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    // Delete one entry by (key, pkey)
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.index_delete(index_id, b"b", b"1").unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(
        txn.count(src, &EncodedRange::all()).unwrap(),
        2,
        "one entry removed"
    );
    txn.commit().unwrap();

    // Delete all entries tied to primary key "1"
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.index_delete_by_primary(index_id, b"1").unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(
        txn.count(src, &EncodedRange::all()).unwrap(),
        1,
        "only (a, 2) remains"
    );
}

#[test]
fn test_index_unique_violation() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("index_unique_violation_db").unwrap();

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
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_code"),
                key_path: KeyPath::parse_single("code").unwrap(),
                unique: true,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.index_put(index_id, b"AAA", b"1", true).unwrap();
    let err = txn.index_put(index_id, b"AAA", b"2", true);
    assert!(err.is_err(), "duplicate unique index key must be rejected");
    txn.commit().unwrap();

    // The failed insert must not have been persisted
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(
        txn.count(
            SourceRef::Index {
                store: store_id,
                index: index_id
            },
            &EncodedRange::all()
        )
        .unwrap(),
        1
    );
}

#[test]
fn test_key_gen_not_found() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "keygen_missing_db");

    let txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let err = txn.key_gen_current(9999);
    assert!(matches!(
        err,
        Err(boa_idb_core::backend::error::BackendError::NotFound(_))
    ));
}

#[test]
fn test_write_in_readonly_fails() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "readonly_write_db");
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert!(txn.put(store_id, b"k", b"v", false).is_err());
    assert!(txn.delete_record(store_id, b"k").is_err());
    assert!(txn.clear(store_id).is_err());
    assert!(txn.key_gen_set(store_id, 5.0).is_err());
    assert!(txn.index_put(9999, b"i", b"p", false).is_err());
    txn.commit().unwrap();
}

#[test]
fn test_readonly_scope_check() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "scope_db");

    // Begin read-only scoped to store_id, but access an out-of-scope store id.
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let out_of_scope = store_id + 1000;
    assert!(txn.get(out_of_scope, b"k").is_err());
    assert!(
        txn.count(SourceRef::Store(out_of_scope), &EncodedRange::all())
            .is_err()
    );
    txn.commit().unwrap();
}

#[test]
fn test_version_abort() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("version_abort_db").unwrap();

    // Commit version 1
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
    txn.commit().unwrap();
    db.flush().unwrap();
    assert_eq!(db.metadata().version, 1);

    // Bump to version 2 but abort
    let store_ids = db
        .metadata()
        .stores
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let mut txn = db
        .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
        .unwrap();
    txn.set_version(2).unwrap();
    txn.abort().unwrap();

    // Version change must be rolled back
    assert_eq!(db.metadata().version, 1);

    // Persisted state also keeps version 1
    let mut txn = db
        .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
        .unwrap();
    txn.set_version(1).unwrap();
    txn.abort().unwrap();
    let txn = db
        .begin(TxnMode::ReadOnly, &[], Durability::Default)
        .unwrap();
    txn.commit().unwrap();
    assert_eq!(db.metadata().version, 1);
}

#[test]
fn test_flush_and_capabilities() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id) = open_items_db(&factory, "flush_db");

    let caps = db.capabilities();
    assert!(caps.snapshot_isolation);
    assert!(caps.durable);
    assert!(caps.concurrent);

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"k", b"v", false).unwrap();
    txn.commit().unwrap();

    db.flush().unwrap();
}

#[test]
fn test_close_returns_ok() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let db = storage.open_database("close_db").unwrap();
    db.close().unwrap();
}

#[test]
fn test_storage_usage_bytes() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();

    assert_eq!(storage.list_databases().unwrap().len(), 0);
    let usage = storage.usage_bytes().unwrap();

    let mut db = storage.open_database("usage_db").unwrap();
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
    let payload = "x".repeat(4096);
    txn.put(store_id, b"payload", payload.as_bytes(), false)
        .unwrap();
    txn.commit().unwrap();

    let usage2 = storage.usage_bytes().unwrap();
    assert!(usage2 > usage, "usage should grow after writing data");
}

#[test]
fn test_key_path_single_and_array_roundtrip() {
    let (factory, tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("keypath_db").unwrap();

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
    txn.create_store(&StoreSpec {
        name: Utf16String::from("with_single"),
        key_path: KeyPath::parse_single("profile.name").unwrap(),
        auto_increment: false,
    })
    .unwrap();
    txn.create_store(&StoreSpec {
        name: Utf16String::from("with_array"),
        key_path: KeyPath::parse_array(&["a", "b.c"]).unwrap(),
        auto_increment: true,
    })
    .unwrap();
    txn.commit().unwrap();

    // Persists across reopen
    drop(storage);
    drop(db);
    let factory2 = SqliteBackendFactory::new(tmp.path());
    let storage2 = factory2
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let db2 = storage2.open_database("keypath_db").unwrap();

    let single = db2
        .metadata()
        .stores
        .iter()
        .find(|s| s.name.to_string() == "with_single")
        .unwrap();
    assert_eq!(
        single.key_path,
        KeyPath::parse_single("profile.name").unwrap()
    );
    assert!(!single.auto_increment);

    let array = db2
        .metadata()
        .stores
        .iter()
        .find(|s| s.name.to_string() == "with_array")
        .unwrap();
    assert_eq!(array.key_path, KeyPath::parse_array(&["a", "b.c"]).unwrap());
    assert!(array.auto_increment);
}

/// Creates a database with a single store that has a non-unique index
/// `by_tag` and returns it with `(store_id, index_id)`.
fn open_indexed_db(
    factory: &SqliteBackendFactory,
    name: &str,
) -> (Box<dyn boa_idb_core::backend::traits::Database>, u64, u64) {
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database(name).unwrap();
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
    let index_id = txn
        .create_index(
            store_id,
            &IndexSpec {
                name: Utf16String::from("by_tag"),
                key_path: KeyPath::parse_single("tag").unwrap(),
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();
    (db, store_id, index_id)
}

#[test]
fn test_delete_record_removes_index_entries() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id, index_id) = open_indexed_db(&factory, "del_idx_db");
    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"1", b"a", false).unwrap();
    txn.index_put(index_id, b"tag-a", b"1", false).unwrap();
    txn.put(store_id, b"2", b"b", false).unwrap();
    txn.index_put(index_id, b"tag-b", b"2", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    assert!(txn.delete_record(store_id, b"1").unwrap());
    txn.commit().unwrap();

    // Index must no longer reference the deleted record.
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.count(src, &EncodedRange::all()).unwrap(), 1);
    let range = EncodedRange::only(b"tag-a".to_vec());
    assert_eq!(txn.count(src, &range).unwrap(), 0);
}

#[test]
fn test_delete_range_removes_index_entries() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id, index_id) = open_indexed_db(&factory, "delrange_idx_db");
    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    for i in 1..=5 {
        let key = format!("k{i}");
        let tag = format!("tag-{i}");
        txn.put(store_id, key.as_bytes(), b"v", false).unwrap();
        txn.index_put(index_id, tag.as_bytes(), key.as_bytes(), false)
            .unwrap();
    }
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    let range = EncodedRange::bound(b"k2".to_vec(), false, b"k4".to_vec(), false);
    assert_eq!(txn.delete_range(store_id, &range).unwrap(), 3);
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.count(src, &EncodedRange::all()).unwrap(), 2);
}

#[test]
fn test_clear_removes_index_entries() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id, index_id) = open_indexed_db(&factory, "clear_idx_db");
    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"1", b"a", false).unwrap();
    txn.index_put(index_id, b"tag-a", b"1", false).unwrap();
    txn.put(store_id, b"2", b"b", false).unwrap();
    txn.index_put(index_id, b"tag-b", b"2", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.clear(store_id).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.count(src, &EncodedRange::all()).unwrap(), 0);
}

#[test]
fn test_put_overwrite_syncs_index_entries() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id, index_id) = open_indexed_db(&factory, "sync_idx_db");
    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    // Insert record 1 with tag-a, record 2 with tag-b.
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"1", b"a", false).unwrap();
    txn.index_put(index_id, b"tag-a", b"1", false).unwrap();
    txn.put(store_id, b"2", b"b", false).unwrap();
    txn.index_put(index_id, b"tag-b", b"2", false).unwrap();
    txn.commit().unwrap();

    // Overwrite record 1's tag from a -> c, mirroring the engine order:
    // new index entries are inserted before the record upsert.
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.index_put(index_id, b"tag-c", b"1", false).unwrap();
    txn.put(store_id, b"1", b"c", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    // tag-a must be gone, tag-c present, tag-b untouched.
    assert_eq!(txn.count(src, &EncodedRange::all()).unwrap(), 2);
    assert_eq!(
        txn.count(src, &EncodedRange::only(b"tag-a".to_vec()))
            .unwrap(),
        0
    );
    assert_eq!(
        txn.count(src, &EncodedRange::only(b"tag-c".to_vec()))
            .unwrap(),
        1
    );
    assert_eq!(
        txn.count(src, &EncodedRange::only(b"tag-b".to_vec()))
            .unwrap(),
        1
    );
}

#[test]
fn test_put_overwrite_drops_removed_index_key() {
    let (factory, _tmp) = test_factory();
    let (mut db, store_id, index_id) = open_indexed_db(&factory, "drop_idx_db");
    let src = SourceRef::Index {
        store: store_id,
        index: index_id,
    };

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"1", b"a", false).unwrap();
    txn.index_put(index_id, b"tag-a", b"1", false).unwrap();
    txn.commit().unwrap();

    // Overwrite record 1 with a value that no longer carries the index key.
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"1", b"new", false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.count(src, &EncodedRange::all()).unwrap(), 0);
}

#[test]
fn test_create_index_duplicate_is_constraint() {
    let (factory, _tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("dup_index_db").unwrap();

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
    txn.create_index(
        store_id,
        &IndexSpec {
            name: Utf16String::from("by_name"),
            key_path: KeyPath::parse_single("name").unwrap(),
            unique: false,
            multi_entry: false,
        },
    )
    .unwrap();
    let err = txn.create_index(
        store_id,
        &IndexSpec {
            name: Utf16String::from("by_name"),
            key_path: KeyPath::parse_single("name").unwrap(),
            unique: false,
            multi_entry: false,
        },
    );
    assert!(matches!(
        err,
        Err(boa_idb_core::backend::error::BackendError::Constraint(_))
    ));
    txn.commit().unwrap();
}

#[test]
fn test_database_name_meta_is_persisted() {
    let (factory, tmp) = test_factory();
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();

    let mut db = storage.open_database("named_db").unwrap();
    assert_eq!(db.metadata().name.to_string(), "named_db");

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
    txn.create_store(&StoreSpec {
        name: Utf16String::from("items"),
        key_path: KeyPath::Empty,
        auto_increment: false,
    })
    .unwrap();
    txn.commit().unwrap();
    drop(db);
    drop(storage);

    // Name survives a reopen of the same storage.
    let factory2 = SqliteBackendFactory::new(tmp.path());
    let storage2 = factory2
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let db2 = storage2.open_database("named_db").unwrap();
    assert_eq!(db2.metadata().name.to_string(), "named_db");
}
