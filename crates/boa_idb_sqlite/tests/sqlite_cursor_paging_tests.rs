//! Keyset-paging equivalence tests (M7-B H-F2).
//!
//! The cursor pages by resume anchor instead of `OFFSET`. These tests walk
//! multiple 128-row pages across the 512-row buffer-drain threshold and
//! assert exact counts, ordering and seek behavior for store, plain-index
//! (duplicate index keys) and unique-index shapes, forward and backward.
//! A mid-iteration `seek` exercises the anchor-reset path.

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::{CursorSeek, IndexSpec, StoreSpec};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Direction, Durability, SourceRef, TxnMode};
use boa_idb_sqlite::SqliteBackendFactory;

/// Record count: several 128-row pages past the 512-row drain threshold.
const ROWS: usize = 3_000;

/// Zero-padded keys sort lexicographically in numeric order.
fn key(i: usize) -> Vec<u8> {
    format!("k{i:06}").into_bytes()
}

/// Creates store `s` with a plain index `by_dup` and a unique index `by_one`.
fn setup() -> (
    tempfile::TempDir,
    Box<dyn boa_idb_core::backend::traits::Storage>,
    u64,
    u64,
    u64,
) {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("paging"))
        .unwrap();
    let mut db = storage.open_database("pagingdb").unwrap();
    let mut txn = db
        .begin(TxnMode::VersionChange, &[], Durability::Default)
        .unwrap();
    txn.set_version(1).unwrap();
    let store = txn
        .create_store(&StoreSpec {
            name: Utf16String::from("s"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .unwrap();
    let plain = txn
        .create_index(
            store,
            &IndexSpec {
                name: Utf16String::from("by_dup"),
                key_path: KeyPath::Empty,
                unique: false,
                multi_entry: false,
            },
        )
        .unwrap();
    let unique = txn
        .create_index(
            store,
            &IndexSpec {
                name: Utf16String::from("by_one"),
                key_path: KeyPath::Empty,
                unique: true,
                multi_entry: false,
            },
        )
        .unwrap();
    txn.commit().unwrap();

    // Fill: one record per key plus same-key index entries on `by_dup`
    // (duplicate index keys, distinct primary keys) and a single entry
    // on the unique index.
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
        .unwrap();
    for i in 0..ROWS {
        txn.begin_request().unwrap();
        txn.put(store, &key(i), b"value", false).unwrap();
        txn.index_put(plain, b"dup", &key(i), false).unwrap();
        txn.commit_request().unwrap();
        if (i + 1) % 500 == 0 {
            txn.commit().unwrap();
            txn = db
                .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
                .unwrap();
        }
    }
    txn.begin_request().unwrap();
    txn.index_put(unique, b"one", &key(0), true).unwrap();
    txn.commit_request().unwrap();
    txn.commit().unwrap();
    db.close().unwrap();
    (tmp, storage, store, plain, unique)
}

/// Walks a cursor to the end, collecting owned keys.
fn walk_keys(
    storage: &dyn boa_idb_core::backend::traits::Storage,
    store: u64,
    source: SourceRef,
    dir: Direction,
) -> Vec<Vec<u8>> {
    let mut db = storage.open_database("pagingdb").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    // Scope the cursor: it borrows the transaction mutably.
    let keys = {
        let mut cursor = txn.scan(source, &EncodedRange::all(), dir, false).unwrap();
        let mut keys = Vec::new();
        if cursor.seek(CursorSeek::First).unwrap() {
            keys.push(cursor.current_key().to_vec());
            while cursor.step(1).unwrap() {
                keys.push(cursor.current_key().to_vec());
            }
        }
        keys
    };
    txn.commit().unwrap();
    db.close().unwrap();
    keys
}

#[test]
fn store_forward_walk_is_complete_and_ordered() {
    let (_tmp, storage, store, _, _) = setup();
    let keys = walk_keys(&*storage, store, SourceRef::Store(store), Direction::Next);
    assert_eq!(keys.len(), ROWS, "every record visited exactly once");
    for (i, k) in keys.iter().enumerate() {
        assert_eq!(k, &key(i), "ascending order without gaps or repeats");
    }
}

#[test]
fn store_backward_walk_is_complete_and_ordered() {
    let (_tmp, storage, store, _, _) = setup();
    let keys = walk_keys(&*storage, store, SourceRef::Store(store), Direction::Prev);
    assert_eq!(keys.len(), ROWS, "every record visited exactly once");
    for (i, k) in keys.iter().enumerate() {
        assert_eq!(k, &key(ROWS - 1 - i), "descending order");
    }
}

#[test]
fn plain_index_walk_covers_duplicate_keys() {
    let (_tmp, storage, store, plain, _) = setup();
    let source = SourceRef::Index {
        store,
        index: plain,
    };
    let keys = walk_keys(&*storage, store, source, Direction::Next);
    assert_eq!(keys.len(), ROWS, "duplicate index keys all visited");
    assert!(
        keys.iter().all(|k| k == b"dup"),
        "every hit carries the duplicate index key"
    );
}

#[test]
fn unique_index_walk_yields_single_group() {
    let (_tmp, storage, store, _, unique) = setup();
    let source = SourceRef::Index {
        store,
        index: unique,
    };
    let keys = walk_keys(&*storage, store, source, Direction::Next);
    assert_eq!(keys, vec![b"one".to_vec()], "one group, smallest pkey");
}

#[test]
fn mid_iteration_seek_resets_and_continues() {
    let (_tmp, storage, store, _, _) = setup();
    let mut db = storage.open_database("pagingdb").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    // Scope the cursor: it borrows the transaction mutably.
    let keys = {
        let mut cursor = txn
            .scan(
                SourceRef::Store(store),
                &EncodedRange::all(),
                Direction::Next,
                false,
            )
            .unwrap();
        assert!(cursor.seek(CursorSeek::First).unwrap());
        assert!(cursor.step(1).unwrap());
        // Reposition mid-iteration: the anchor must reset, not resume.
        assert!(cursor.seek(CursorSeek::Key(key(1500))).unwrap());
        let mut keys = vec![cursor.current_key().to_vec()];
        while cursor.step(1).unwrap() {
            keys.push(cursor.current_key().to_vec());
        }
        keys
    };
    txn.commit().unwrap();
    db.close().unwrap();
    assert_eq!(keys.len(), ROWS - 1500, "tail after seek target");
    for (i, k) in keys.iter().enumerate() {
        assert_eq!(k, &key(1500 + i), "ascending order from seek target");
    }
}
