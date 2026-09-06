//! Blob lifecycle tests: externalized values survive commit, vanish on
//! rollback/abort, and orphaned files are collected at commit.

use boa_idb_core::backend::traits::{BackendFactory, Database};
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, SourceRef, TxnMode};
use boa_idb_sqlite::SqliteBackendFactory;
use boa_idb_sqlite::naming::{db_hash_from_filename, db_name_to_filename};
use std::path::Path;

fn open_items_db(factory: &SqliteBackendFactory, db_name: &str) -> Box<dyn Database> {
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database(db_name).unwrap();
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
    // `metadata()` is a snapshot: synchronize the view.
    db.flush().unwrap();
    db
}

/// Counts `.bin` blob files anywhere under the storage root.
///
/// (The storage-key directory itself is hashed, so walk the whole root.)
fn blob_file_count(root: &Path) -> usize {
    let mut count = 0;
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "bin") {
                count += 1;
            }
        }
    }
    count
}

fn big_value(seed: u8) -> Vec<u8> {
    vec![seed; 512 * 1024]
}

#[test]
fn test_committed_blob_reads_back_and_file_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let mut db = open_items_db(&factory, "blob_commit_db");
    let store_id = db.metadata().stores[0].id;

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"k", &big_value(1), false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();

    assert_eq!(blob_file_count(tmp.path()), 1);

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"k").unwrap(), Some(big_value(1)));
    txn.commit().unwrap();
}

#[test]
fn test_rollback_removes_reference_and_file() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let mut db = open_items_db(&factory, "blob_rollback_db");
    let store_id = db.metadata().stores[0].id;

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"k", &big_value(2), false).unwrap();
    txn.rollback_request().unwrap();
    txn.commit().unwrap();

    // Neither the row nor the file may survive.
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"k").unwrap(), None);
    assert_eq!(
        txn.count(SourceRef::Store(store_id), &EncodedRange::all())
            .unwrap(),
        0
    );
    txn.commit().unwrap();
    assert_eq!(blob_file_count(tmp.path()), 0);
}

#[test]
fn test_delete_collects_orphan_at_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let mut db = open_items_db(&factory, "blob_delete_db");
    let store_id = db.metadata().stores[0].id;

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"k", &big_value(5), false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();
    assert_eq!(blob_file_count(tmp.path()), 1);

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    assert!(txn.delete_record(store_id, b"k").unwrap());
    txn.commit_request().unwrap();
    txn.commit().unwrap();

    assert_eq!(blob_file_count(tmp.path()), 0);
}

#[test]
fn test_delete_store_collects_orphans_at_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let mut db = open_items_db(&factory, "blob_delete_store_db");
    let store_id = db.metadata().stores[0].id;

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"a", &big_value(6), false).unwrap();
    txn.put(store_id, b"b", &big_value(7), false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();
    assert_eq!(blob_file_count(tmp.path()), 2);

    // Deleting the store cascades over its records: their blob files must
    // be collected at commit.
    let store_ids = db
        .metadata()
        .stores
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let mut txn = db
        .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.delete_store(store_id).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();

    assert_eq!(blob_file_count(tmp.path()), 0);
}

#[test]
fn test_delete_store_rollback_keeps_blob_files() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let mut db = open_items_db(&factory, "blob_delete_store_rb_db");
    let store_id = db.metadata().stores[0].id;

    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"a", &big_value(8), false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();
    assert_eq!(blob_file_count(tmp.path()), 1);

    // A rolled-back store deletion must not touch the file.
    let store_ids = db
        .metadata()
        .stores
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let mut txn = db
        .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.delete_store(store_id).unwrap();
    txn.rollback_request().unwrap();
    txn.commit().unwrap();

    assert_eq!(blob_file_count(tmp.path()), 1);
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"a").unwrap(), Some(big_value(8)));
    txn.commit().unwrap();
}

/// Dropping an uncommitted transaction is an implicit abort: its blob files
/// must not leak.
#[test]
fn test_drop_without_commit_removes_blob_file() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let mut db = open_items_db(&factory, "blob_drop_db");
    let store_id = db.metadata().stores[0].id;

    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store_id, b"k", &big_value(9), false).unwrap();
        txn.commit_request().unwrap();
        // `txn` dropped here without commit/abort.
    }

    assert_eq!(blob_file_count(tmp.path()), 0);
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"k").unwrap(), None);
    txn.commit().unwrap();
}

#[test]
fn test_overwrite_collects_orphan_at_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let mut db = open_items_db(&factory, "blob_overwrite_db");
    let store_id = db.metadata().stores[0].id;

    // Big value → external file.
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"k", &big_value(4), false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();
    assert_eq!(blob_file_count(tmp.path()), 1);

    // Overwrite with an inline value → orphan collected at commit.
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"k", b"small", false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();

    assert_eq!(blob_file_count(tmp.path()), 0);
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Default)
        .unwrap();
    assert_eq!(txn.get(store_id, b"k").unwrap(), Some(b"small".to_vec()));
    txn.commit().unwrap();
}

#[test]
fn test_open_sweeps_crash_orphan_blob() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let key = boa_idb_core::proto::StorageKey::new("test");
    let storage_root = factory.storage_dir(&key);
    let db_file = db_name_to_filename(&"orphan_db".encode_utf16().collect::<Vec<_>>());
    let db_hash = db_hash_from_filename(&db_file).unwrap();
    let orphan = storage_root
        .join("blobs")
        .join(db_hash)
        .join("aa/orphan.bin");
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, b"orphaned before sqlite commit").unwrap();

    let storage = factory.open_storage(&key).unwrap();
    let _db = storage.open_database("orphan_db").unwrap();

    assert!(!orphan.exists(), "open must collect unreachable blob files");
}

/// Crash orphans are swept on first open after restart; repeat opens in the
/// same process skip the rescan (M7-B H-F4). In-process blob files are
/// lifecycle-managed, so only a restart can introduce orphans.
#[test]
fn test_repeat_open_skips_rescan_but_restart_sweeps() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let key = boa_idb_core::proto::StorageKey::new("test");

    // Database with one referenced (externalized) blob.
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("resweep_db").unwrap();
    let mut txn = db
        .begin(TxnMode::VersionChange, &[], Durability::Default)
        .unwrap();
    txn.set_version(1).unwrap();
    let store_id = txn
        .create_store(&StoreSpec {
            name: Utf16String::from_str("items"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    txn.commit().unwrap();
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store_id], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn.put(store_id, b"k", &big_value(9), false).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();
    drop(db);
    assert_eq!(blob_file_count(tmp.path()), 1);

    // Plant a crash orphan under this database's blob dir.
    let db_file = db_name_to_filename(&"resweep_db".encode_utf16().collect::<Vec<_>>());
    let db_hash = db_hash_from_filename(&db_file).unwrap();
    let orphan = factory
        .storage_dir(&key)
        .join("blobs")
        .join(db_hash)
        .join("ab/orphan-b.bin");
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, b"simulated crash leftover").unwrap();

    // Same storage: the sweep already ran, the rescan is skipped.
    let mut db = storage.open_database("resweep_db").unwrap();
    assert!(orphan.exists(), "same-process reopen must skip the rescan");
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store_id], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store_id, b"k").unwrap(), Some(big_value(9)));
    txn.commit().unwrap();
    drop(db);

    // Fresh storage (restart): first open sweeps again, keeping the referent.
    drop(storage);
    let storage = factory.open_storage(&key).unwrap();
    let _db = storage.open_database("resweep_db").unwrap();
    assert!(!orphan.exists(), "restart open must sweep crash orphans");
    assert_eq!(
        blob_file_count(tmp.path()),
        1,
        "referenced blob must survive the sweep"
    );
}
