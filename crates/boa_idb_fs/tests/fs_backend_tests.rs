//! Integration tests for the filesystem backend (M6-A).

#![allow(clippy::float_cmp, clippy::too_many_lines)]

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::{CursorSeek, IndexSpec, StoreSpec};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Direction, Durability, SourceRef, StorageKey, TxnMode};
use boa_idb_fs::{
    CountingSyncHooks, FLAG_COMMIT, FsBackendFactory, WalFrame, WalOp, decode_frame, encode_frame,
    recover_committed_frames,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use tempfile::tempdir;

fn factory(root: &std::path::Path) -> FsBackendFactory {
    FsBackendFactory::new(root)
}

#[test]
fn crud_put_get_persists_across_reopen() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("https://example.test");
    let store = {
        let storage = factory(dir.path()).open_storage(&key).unwrap();
        let mut db = storage.open_database("mydb").unwrap();
        let mut txn = db
            .begin(TxnMode::VersionChange, &[], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        let store = txn
            .create_store(&StoreSpec {
                name: Utf16String::from_str("s"),
                key_path: KeyPath::Empty,
                auto_increment: false,
            })
            .unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
        drop(db);

        let mut db = storage.open_database("mydb").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"k1", b"v1", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
        store
    };
    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let mut db = storage.open_database("mydb").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"k1").unwrap(), Some(b"v1".to_vec()));
}

#[test]
fn abort_and_savepoint_rollback_do_not_persist() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("origin");
    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let store = {
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
        store
    };
    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"a", b"1", false).unwrap();
        txn.rollback_request().unwrap();
        txn.abort().unwrap();
    }
    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"a").unwrap(), None);
}

#[test]
fn torn_wal_tail_is_discarded_on_open() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("torn");
    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let store = {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::VersionChange, &[], Durability::Strict)
            .unwrap();
        let store = txn
            .create_store(&StoreSpec {
                name: Utf16String::from_str("s"),
                key_path: KeyPath::Empty,
                auto_increment: false,
            })
            .unwrap();
        txn.commit().unwrap();
        store
    };
    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"ok", b"yes", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }

    let wal = find_wal(dir.path());
    let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
    let good = encode_frame(&WalFrame {
        txn_seq: 99,
        flags: FLAG_COMMIT,
        ops: vec![WalOp::Put {
            store,
            key: b"ghost".to_vec(),
            value: b"no".to_vec(),
        }],
    })
    .unwrap();
    let mut bad = good;
    let last = bad.len() - 1;
    bad[last] ^= 0xff;
    file.write_all(&bad).unwrap();
    file.write_all(&[0xde, 0xad]).unwrap();
    drop(file);

    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"ok").unwrap(), Some(b"yes".to_vec()));
    assert_eq!(txn.get(store, b"ghost").unwrap(), None);
}

#[test]
fn strict_commit_syncs_wal_relaxed_does_not() {
    let dir = tempdir().unwrap();
    let hooks = CountingSyncHooks::new();
    let factory = FsBackendFactory::new(dir.path()).with_sync_hooks(hooks.clone());
    let key = StorageKey::new("sync");
    let storage = factory.open_storage(&key).unwrap();
    let store = {
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
        store
    };
    let before = hooks.file_sync_count();

    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"r", b"1", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    let after_relaxed = hooks.file_sync_count();
    assert_eq!(after_relaxed, before);

    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"s", b"2", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    assert!(hooks.file_sync_count() > after_relaxed);
}

#[test]
fn second_open_fails_while_lock_held() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("lock");
    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let db = storage.open_database("db").unwrap();
    let err = factory(dir.path())
        .open_storage(&key)
        .unwrap()
        .open_database("db")
        .err()
        .expect("second open must fail");
    assert!(matches!(err, BackendError::Locked));
    drop(db);
    assert!(
        factory(dir.path())
            .open_storage(&key)
            .unwrap()
            .open_database("db")
            .is_ok()
    );
}

#[test]
fn max_keys_in_memory_boundary() {
    let dir = tempdir().unwrap();
    let factory = FsBackendFactory::new(dir.path()).with_max_keys_in_memory(2);
    let key = StorageKey::new("quota");
    let storage = factory.open_storage(&key).unwrap();
    let store = {
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
        store
    };

    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"1", b"a", false).unwrap();
        txn.put(store, b"2", b"b", false).unwrap();
        let err = txn.put(store, b"3", b"c", false).unwrap_err();
        assert!(matches!(err, BackendError::QuotaExceeded { .. }));
        txn.rollback_request().unwrap();
        txn.abort().unwrap();
    }

    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"1").unwrap(), None);
    assert_eq!(txn.get(store, b"2").unwrap(), None);
}

#[test]
fn subprocess_crash_recovers_committed_prefix() {
    // Windows-stable seam: committed WAL + torn append, then reopen.
    // Dedicated kill-worker for full R8.5.1 matrix is deferred to M6-B.
    let dir = tempdir().unwrap();
    let key = StorageKey::new("crash");
    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let store = {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::VersionChange, &[], Durability::Strict)
            .unwrap();
        let store = txn
            .create_store(&StoreSpec {
                name: Utf16String::from_str("s"),
                key_path: KeyPath::Empty,
                auto_increment: false,
            })
            .unwrap();
        txn.commit().unwrap();
        store
    };
    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"c", b"committed", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    let wal = find_wal(dir.path());
    let mut f = OpenOptions::new().append(true).open(&wal).unwrap();
    f.write_all(b"TORN").unwrap();
    drop(f);

    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"c").unwrap(), Some(b"committed".to_vec()));
}

#[test]
fn wal_property_garbage_never_panics_integration() {
    let bytes = vec![0u8, 1, 2, 3, 4, 5, 255, 128];
    let _ = recover_committed_frames(&bytes);
    let _ = decode_frame(&bytes);
}

#[test]
fn schema_index_cursor_keygen_parity_persists() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("https://parity.test");
    let (store, index) = {
        let storage = factory(dir.path()).open_storage(&key).unwrap();
        let mut db = storage.open_database("parity").unwrap();
        let mut txn = db
            .begin(TxnMode::VersionChange, &[], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        let store = txn
            .create_store(&StoreSpec {
                name: Utf16String::from_str("s"),
                key_path: KeyPath::Empty,
                auto_increment: true,
            })
            .unwrap();
        let index = txn
            .create_index(
                store,
                &IndexSpec {
                    name: Utf16String::from_str("byName"),
                    key_path: KeyPath::Empty,
                    unique: true,
                    multi_entry: false,
                },
            )
            .unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
        drop(db);

        let mut db = storage.open_database("parity").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.key_gen_set(store, 10.0).unwrap();
        txn.put(store, b"pk1", b"v1", false).unwrap();
        txn.put(store, b"pk2", b"v2", false).unwrap();
        txn.index_put(index, b"n1", b"pk1", true).unwrap();
        txn.index_put(index, b"n2", b"pk2", true).unwrap();
        assert!(txn.index_put(index, b"n1", b"pk3", true).is_err());
        txn.commit_request().unwrap();
        txn.commit().unwrap();
        (store, index)
    };

    let storage = factory(dir.path()).open_storage(&key).unwrap();
    let mut db = storage.open_database("parity").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.key_gen_current(store).unwrap(), 10.0);
    assert_eq!(txn.get(store, b"pk1").unwrap(), Some(b"v1".to_vec()));

    {
        let mut cursor = txn
            .scan(
                SourceRef::Store(store),
                &EncodedRange::all(),
                Direction::Next,
                false,
            )
            .unwrap();
        assert!(cursor.seek(CursorSeek::First).unwrap());
        assert_eq!(cursor.current_key(), b"pk1");
        assert_eq!(cursor.current_value(), Some(b"v1".as_slice()));
        assert!(cursor.step(1).unwrap());
        assert_eq!(cursor.current_key(), b"pk2");
        assert!(!cursor.step(1).unwrap());
    }
    {
        let mut cursor = txn
            .scan(
                SourceRef::Index { store, index },
                &EncodedRange::all(),
                Direction::Next,
                true,
            )
            .unwrap();
        assert!(cursor.seek(CursorSeek::First).unwrap());
        assert_eq!(cursor.current_key(), b"n1");
        assert_eq!(cursor.current_primary_key(), b"pk1");
    }
}

fn find_wal(root: &std::path::Path) -> std::path::PathBuf {
    for sk in fs::read_dir(root).unwrap().flatten() {
        for db in fs::read_dir(sk.path()).unwrap().flatten() {
            let wal = db.path().join("wal").join("000001.log");
            if wal.exists() {
                return wal;
            }
        }
    }
    panic!("wal not found under {}", root.display());
}
