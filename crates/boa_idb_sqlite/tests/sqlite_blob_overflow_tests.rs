//! Blob overflow tests: large values stored as external files.

use boa_idb_core::backend::traits::{BackendFactory, Database};
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, TxnMode};
use boa_idb_sqlite::SqliteBackendFactory;

fn setup_db(factory: &SqliteBackendFactory) -> (Box<dyn Database>, u64) {
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("blob_db").unwrap();

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
            name: Utf16String::from("blobs"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    txn.commit().unwrap();

    (db, store_id)
}

#[test]
fn test_insert_512kib_value() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let value = vec![0xABu8; 512 * 1024]; // 512 KiB

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"big", &value, false).unwrap();
    txn.commit().unwrap();

    // Read back
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let val = txn.get(store_id, b"big").unwrap();
    assert_eq!(val, Some(value));
}

#[test]
fn test_insert_2mib_value() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let value = vec![0xCDu8; 2 * 1024 * 1024]; // 2 MiB

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"big2", &value, false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let val = txn.get(store_id, b"big2").unwrap();
    assert_eq!(val, Some(value));
}

#[test]
fn test_insert_10mib_value() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let value = vec![0xEFu8; 10 * 1024 * 1024]; // 10 MiB

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"huge", &value, false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let val = txn.get(store_id, b"huge").unwrap();
    assert_eq!(val, Some(value));
}

#[test]
fn test_blob_files_created_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let value = vec![0x42u8; 512 * 1024]; // > 256 KiB threshold

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"external", &value, false).unwrap();
    txn.commit().unwrap();

    // Check that blob files exist (under the storage-key directory)
    let key = boa_idb_core::proto::StorageKey::new("test");
    let blobs_dir = factory.storage_dir(&key).join("blobs");
    assert!(blobs_dir.exists(), "blobs directory should exist");

    // Walk the blobs directory to find .bin files
    let mut found_bin = false;
    walk_dir(&blobs_dir, &mut found_bin);
    assert!(found_bin, "should find .bin blob files");
}

/// Recursively scans `dir`, setting `found` when a `.bin` file is present.
fn walk_dir(dir: &std::path::Path, found: &mut bool) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_dir(&path, found);
            } else if path.extension().is_some_and(|e| e == "bin") {
                *found = true;
            }
        }
    }
}

/// Recursively finds the first `.bin` file under `dir`.
fn find_bin(dir: &std::path::Path, out: &mut Option<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                find_bin(&path, out);
            } else if path.extension().is_some_and(|e| e == "bin") {
                *out = Some(path);
            }
        }
    }
}

#[test]
fn test_rollback_removes_blob_references() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let value = vec![0x99u8; 512 * 1024]; // > 256 KiB

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"rolled_back", &value, false).unwrap();
    txn.abort().unwrap();

    // Verify no records in database
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let val = txn.get(store_id, b"rolled_back").unwrap();
    assert_eq!(val, None);
}

#[test]
fn test_mixed_inline_and_external() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let small_value = b"small".to_vec();
    let big_value = vec![0xBBu8; 512 * 1024];

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"small", &small_value, false).unwrap();
    txn.put(store_id, b"big", &big_value, false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"small").unwrap(), Some(small_value));
    assert_eq!(txn.get(store_id, b"big").unwrap(), Some(big_value));
}

#[test]
fn test_blob_roundtrip_after_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let value = vec![0x5Au8; 2 * 1024 * 1024];
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"big", &value, false).unwrap();
    txn.commit().unwrap();

    drop(db);

    // Reopen with a fresh factory and read the externalized value back.
    let factory2 = SqliteBackendFactory::new(tmp.path());
    let storage2 = factory2
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db2 = storage2.open_database("blob_db").unwrap();
    let store_id2 = db2.metadata().stores[0].id;

    let mut txn = db2
        .begin(TxnMode::ReadOnly, &[store_id2], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id2, b"big").unwrap(), Some(value));
}

#[test]
fn test_corrupted_blob_file_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let value = vec![0x77u8; 512 * 1024];
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"big", &value, false).unwrap();
    txn.commit().unwrap();

    // Corrupt the blob file on disk (tamper with its content).
    let key = boa_idb_core::proto::StorageKey::new("test");
    let blobs_root = factory.storage_dir(&key).join("blobs");
    let mut target = None;
    find_bin(&blobs_root, &mut target);
    let path = target.expect("blob file exists on disk");
    std::fs::write(&path, vec![0xEEu8; 512 * 1024]).unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let err = txn.get(store_id, b"big").unwrap_err();
    assert!(matches!(
        err,
        boa_idb_core::backend::error::BackendError::Corrupted(_)
    ));
}

#[test]
fn test_blob_value_readable_via_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let (mut db, store_id) = setup_db(&factory);

    let small = b"small_value".to_vec();
    let big = vec![0x6Du8; 512 * 1024];

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.put(store_id, b"small", &small, false).unwrap();
    txn.put(store_id, b"big", &big, false).unwrap();
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    let range = boa_idb_core::key::range::EncodedRange::all();
    let mut cursor = txn
        .scan(
            boa_idb_core::proto::SourceRef::Store(store_id),
            &range,
            boa_idb_core::proto::Direction::Next,
            false,
        )
        .unwrap();
    assert!(
        cursor
            .seek(boa_idb_core::backend::types::CursorSeek::First)
            .unwrap()
    );
    assert_eq!(cursor.current_key(), b"big");
    assert_eq!(cursor.current_value(), Some(big.as_slice()));
    assert!(cursor.step(1).unwrap());
    assert_eq!(cursor.current_key(), b"small");
    assert_eq!(cursor.current_value(), Some(small.as_slice()));
}
