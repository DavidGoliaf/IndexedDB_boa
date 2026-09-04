//! Differential parity: memory vs filesystem backends (M6-B3 / R13.4.*).
//!
//! Fixed scenarios cover CRUD, schema/index/cursors, abort/savepoint, and
//! durable reopen. Durability differences are limited to what FS guarantees
//! after `Durability::Strict` commit + reopen.

#![allow(clippy::float_cmp, clippy::too_many_lines)]

use boa_idb_core::backend::traits::{BackendFactory, BackendTxn};
use boa_idb_core::backend::types::{CursorSeek, IndexSpec, StoreSpec};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Direction, Durability, SourceRef, StorageKey, TxnMode};
use boa_idb_fs::FsBackendFactory;
use boa_idb_memory::MemoryBackendFactory;
use std::collections::BTreeMap;
use tempfile::tempdir;

fn dump_store(txn: &mut dyn BackendTxn, store: u64) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut cursor = txn
        .scan(
            SourceRef::Store(store),
            &EncodedRange::all(),
            Direction::Next,
            false,
        )
        .unwrap();
    let mut map = BTreeMap::new();
    let mut active = cursor.seek(CursorSeek::First).unwrap();
    while active {
        map.insert(
            cursor.current_key().to_vec(),
            cursor.current_value().unwrap().to_vec(),
        );
        active = cursor.step(1).unwrap();
    }
    map
}

fn dump_index(txn: &mut dyn BackendTxn, store: u64, index: u64) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut cursor = txn
        .scan(
            SourceRef::Index { store, index },
            &EncodedRange::all(),
            Direction::Next,
            false,
        )
        .unwrap();
    let mut out = Vec::new();
    let mut active = cursor.seek(CursorSeek::First).unwrap();
    while active {
        out.push((
            cursor.current_key().to_vec(),
            cursor.current_primary_key().to_vec(),
        ));
        active = cursor.step(1).unwrap();
    }
    out
}

fn with_backends(f: impl Fn(&dyn BackendFactory, &StorageKey)) {
    let mem = MemoryBackendFactory::new();
    let dir = tempdir().unwrap();
    let fs = FsBackendFactory::new(dir.path());
    let key = StorageKey::new("https://diff.example");
    f(&mem, &key);
    f(&fs, &key);
}

#[test]
fn crud_schema_index_cursor_parity() {
    with_backends(|factory, key| {
        let storage = factory.open_storage(key).unwrap();
        let (store, index) = {
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
            let index = txn
                .create_index(
                    store,
                    &IndexSpec {
                        name: Utf16String::from_str("ix"),
                        key_path: KeyPath::Empty,
                        unique: false,
                        multi_entry: false,
                    },
                )
                .unwrap();
            txn.commit().unwrap();
            (store, index)
        };
        {
            let mut db = storage.open_database("db").unwrap();
            let mut txn = db
                .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
                .unwrap();
            txn.begin_request().unwrap();
            txn.put(store, b"a", b"1", false).unwrap();
            txn.put(store, b"b", b"2", false).unwrap();
            txn.index_put(index, b"ia", b"a", false).unwrap();
            txn.index_put(index, b"ib", b"b", false).unwrap();
            txn.commit_request().unwrap();
            txn.commit().unwrap();
        }
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
            .unwrap();
        assert_eq!(txn.get(store, b"a").unwrap(), Some(b"1".to_vec()));
        let records = dump_store(&mut *txn, store);
        assert_eq!(records.len(), 2);
        let idx = dump_index(&mut *txn, store, index);
        assert_eq!(idx.len(), 2);
    });
}

#[test]
fn abort_and_savepoint_parity() {
    with_backends(|factory, key| {
        let storage = factory.open_storage(key).unwrap();
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
            txn.put(store, b"x", b"1", false).unwrap();
            txn.begin_request().unwrap();
            txn.put(store, b"y", b"2", false).unwrap();
            txn.rollback_request().unwrap();
            txn.commit_request().unwrap();
            txn.commit().unwrap();
        }
        {
            let mut db = storage.open_database("db").unwrap();
            let mut txn = db
                .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
                .unwrap();
            txn.begin_request().unwrap();
            txn.put(store, b"z", b"3", false).unwrap();
            txn.abort().unwrap();
        }
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
            .unwrap();
        assert_eq!(txn.get(store, b"x").unwrap(), Some(b"1".to_vec()));
        assert_eq!(txn.get(store, b"y").unwrap(), None);
        assert_eq!(txn.get(store, b"z").unwrap(), None);
    });
}

#[test]
fn durability_reopen_parity_on_fs_only_path() {
    // Memory has no durable reopen; FS must retain Strict commits across open.
    let dir = tempdir().unwrap();
    let factory = FsBackendFactory::new(dir.path());
    let key = StorageKey::new("reopen");
    let store = {
        let storage = factory.open_storage(&key).unwrap();
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
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"k", b"v", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
        store
    };
    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    assert_eq!(txn.get(store, b"k").unwrap(), Some(b"v".to_vec()));
}

#[test]
fn memory_fs_record_map_matches_after_same_ops() {
    fn run(factory: &dyn BackendFactory) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let key = StorageKey::new("same-ops");
        let storage = factory.open_storage(&key).unwrap();
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
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"1", b"a", false).unwrap();
        txn.put(store, b"2", b"b", false).unwrap();
        txn.delete_record(store, b"1").unwrap();
        txn.put(store, b"3", b"c", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
        let mut txn = db
            .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
            .unwrap();
        dump_store(&mut *txn, store)
    }

    let mem = MemoryBackendFactory::new();
    let dir = tempdir().unwrap();
    let fs = FsBackendFactory::new(dir.path());
    assert_eq!(run(&mem), run(&fs));
}
