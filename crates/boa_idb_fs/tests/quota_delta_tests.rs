//! Quota projection with incremental delta (M7-B H-F1).
//!
//! The per-`put` pending rescan was replaced by a maintained net-insert
//! delta with savepoint snapshots. These tests pin the exact boundary
//! behavior across puts, overwrites, deletes and (nested) savepoint
//! rollback: without snapshot restore, rolled-back inserts would falsely
//! trip quota on later puts.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, StorageKey, TxnMode};
use boa_idb_fs::FsBackendFactory;
use tempfile::tempdir;

/// Creates database `db` with one store `s`; returns the store id.
fn setup(
    factory: &FsBackendFactory,
    key: &StorageKey,
) -> (Box<dyn boa_idb_core::backend::traits::Storage>, u64) {
    let storage = factory.open_storage(key).unwrap();
    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::VersionChange, &[], Durability::Relaxed)
        .unwrap();
    let store = txn
        .create_store(&StoreSpec {
            name: Utf16String::from_str("s"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    txn.commit().unwrap();
    (storage, store)
}

#[test]
fn quota_boundary_with_deletes_and_overwrites() {
    let dir = tempdir().unwrap();
    let factory = FsBackendFactory::new(dir.path()).with_max_keys_in_memory(3);
    let key = StorageKey::new("quota-delta");
    let (storage, store) = setup(&factory, &key);

    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
        .unwrap();
    txn.put(store, b"k1", b"v1", false).unwrap();
    txn.put(store, b"k2", b"v2", false).unwrap();
    txn.put(store, b"k3", b"v3", false).unwrap();
    // Exact boundary: 3/3 full, the 4th reports needed 4 of 3.
    let err = txn.put(store, b"k4", b"v4", false).unwrap_err();
    assert!(
        matches!(
            err,
            BackendError::QuotaExceeded {
                needed: 4,
                available: 3
            }
        ),
        "exact quota error shape: {err:?}"
    );
    // Overwrites grow nothing.
    txn.put(store, b"k1", b"v1-new", false).unwrap();
    // Delete frees one slot; the insert fits exactly.
    txn.delete_record(store, b"k2").unwrap();
    txn.put(store, b"k4", b"v4", false).unwrap();
    let err = txn.put(store, b"k5", b"v5", false).unwrap_err();
    assert!(
        matches!(
            err,
            BackendError::QuotaExceeded {
                needed: 4,
                available: 3
            }
        ),
        "boundary holds after delete+insert: {err:?}"
    );
    txn.commit().unwrap();

    // Data landed exactly as counted.
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"k1").unwrap(), Some(b"v1-new".to_vec()));
    assert_eq!(txn.get(store, b"k2").unwrap(), None);
    assert_eq!(txn.get(store, b"k4").unwrap(), Some(b"v4".to_vec()));
    txn.commit().unwrap();
}

#[test]
fn quota_delta_restored_on_savepoint_rollback() {
    let dir = tempdir().unwrap();
    let factory = FsBackendFactory::new(dir.path()).with_max_keys_in_memory(3);
    let key = StorageKey::new("quota-rollback");
    let (storage, store) = setup(&factory, &key);

    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
        .unwrap();
    // Rolled-back insert must not poison later projections: without the
    // frame-start delta snapshot, k3 below would falsely exceed quota.
    txn.begin_request().unwrap();
    txn.put(store, b"k1", b"v1", false).unwrap();
    txn.rollback_request().unwrap();
    txn.put(store, b"k1", b"v1", false).unwrap();
    txn.put(store, b"k2", b"v2", false).unwrap();
    txn.put(store, b"k3", b"v3", false).unwrap();
    let err = txn.put(store, b"k4", b"v4", false).unwrap_err();
    assert!(
        matches!(
            err,
            BackendError::QuotaExceeded {
                needed: 4,
                available: 3
            }
        ),
        "exact 3/3 boundary after rollback: {err:?}"
    );
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"k3").unwrap(), Some(b"v3".to_vec()));
    txn.commit().unwrap();
}

#[test]
fn quota_delta_nested_savepoints() {
    let dir = tempdir().unwrap();
    let factory = FsBackendFactory::new(dir.path()).with_max_keys_in_memory(4);
    let key = StorageKey::new("quota-nested");
    let (storage, store) = setup(&factory, &key);

    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
        .unwrap();
    txn.put(store, b"k1", b"v1", false).unwrap();
    txn.begin_request().unwrap();
    txn.put(store, b"k2", b"v2", false).unwrap();
    txn.begin_request().unwrap();
    txn.put(store, b"k3", b"v3", false).unwrap();
    // Inner commit merges: the projection keeps all three inserts.
    txn.commit_request().unwrap();
    txn.put(store, b"k4", b"v4", false).unwrap();
    let err = txn.put(store, b"k5", b"v5", false).unwrap_err();
    assert!(
        matches!(
            err,
            BackendError::QuotaExceeded {
                needed: 5,
                available: 4
            }
        ),
        "4/4 boundary with merged frames: {err:?}"
    );
    // Outer rollback undoes both levels; the projection restarts from k1.
    txn.rollback_request().unwrap();
    txn.put(store, b"k2", b"v2", false).unwrap();
    txn.put(store, b"k3", b"v3", false).unwrap();
    txn.put(store, b"k4", b"v4", false).unwrap();
    let err = txn.put(store, b"k5", b"v5", false).unwrap_err();
    assert!(
        matches!(
            err,
            BackendError::QuotaExceeded {
                needed: 5,
                available: 4
            }
        ),
        "exact boundary after outer rollback: {err:?}"
    );
    txn.commit().unwrap();

    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    for (k, v) in [
        (b"k1", b"v1"),
        (b"k2", b"v2"),
        (b"k3", b"v3"),
        (b"k4", b"v4"),
    ] {
        assert_eq!(
            txn.get(store, k).unwrap(),
            Some(v.to_vec()),
            "record intact"
        );
    }
    txn.commit().unwrap();
}
