//! M6-B1: segments, compaction, and MVCC snapshot proofs.

#![allow(clippy::float_cmp)]

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, StorageKey, TxnMode};
use boa_idb_fs::{CompactConfig, FsBackendFactory, SnapshotMeter};
use std::fs;
use tempfile::tempdir;

fn setup_store(root: &std::path::Path, name: &str) -> u64 {
    let storage = FsBackendFactory::new(root)
        .open_storage(&StorageKey::new(name))
        .unwrap();
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
}

fn find_seg_dir(root: &std::path::Path) -> std::path::PathBuf {
    for sk in fs::read_dir(root).unwrap().flatten() {
        for db in fs::read_dir(sk.path()).unwrap().flatten() {
            let seg = db.path().join("seg");
            if seg.is_dir() {
                return seg;
            }
        }
    }
    panic!("seg dir not found");
}

fn count_segments(root: &std::path::Path) -> usize {
    let seg = find_seg_dir(root);
    fs::read_dir(seg)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "seg"))
        .count()
}

#[test]
fn compaction_by_frame_threshold_writes_segment_and_reopens() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("compact-frames");
    let factory = FsBackendFactory::new(dir.path()).with_compact_config(CompactConfig {
        wal_bytes: u64::MAX,
        wal_frames: 3,
    });
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
        store
    };

    let storage = factory.open_storage(&key).unwrap();
    for i in 0..3u8 {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, &[b'k', i], &[b'v', i], false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    assert!(
        count_segments(dir.path()) >= 1,
        "expected at least one segment after threshold"
    );
    drop(storage);

    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("db").unwrap();
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    for i in 0..3u8 {
        assert_eq!(txn.get(store, &[b'k', i]).unwrap(), Some(vec![b'v', i]));
    }
}

#[test]
fn readonly_snapshot_is_structural_not_deep() {
    let dir = tempdir().unwrap();
    let meter = SnapshotMeter::new();
    let factory = FsBackendFactory::new(dir.path()).with_snapshot_meter(meter.clone());
    let key = StorageKey::new("meter");
    let store = setup_store(dir.path(), "meter");

    let storage = factory.open_storage(&key).unwrap();
    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        for i in 0..5_000u32 {
            let k = i.to_le_bytes();
            txn.put(store, &k, &k, false).unwrap();
        }
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }

    let before = meter.snapshot();
    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
            .unwrap();
        assert_eq!(
            txn.get(store, &0u32.to_le_bytes()).unwrap(),
            Some(0u32.to_le_bytes().to_vec())
        );
        let _ = txn;
    }
    let after = meter.snapshot();
    assert!(after.1 > before.1, "snapshot_begins must increase");
    assert!(after.0 > before.0, "structural_clones must increase");
    assert_eq!(
        after.2, before.2,
        "deep_record_walks must stay unchanged on RO begin"
    );
}

#[test]
fn readonly_snapshot_retains_segment_until_drop() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("retain");
    let factory = FsBackendFactory::new(dir.path()).with_compact_config(CompactConfig {
        wal_bytes: u64::MAX,
        wal_frames: 1,
    });
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
        store
    };

    let storage = factory.open_storage(&key).unwrap();
    {
        let mut db = storage.open_database("db").unwrap();
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"a", b"1", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }

    let tip = {
        let mut paths: Vec<_> = fs::read_dir(find_seg_dir(dir.path()))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "seg"))
            .collect();
        paths.sort();
        paths.pop().expect("tip segment after compact")
    };
    assert!(tip.exists());

    let mut db = storage.open_database("db").unwrap();
    let ro = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();

    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"b", b"2", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    assert!(
        tip.exists(),
        "prior tip segment must survive while readonly snapshot is live"
    );
    drop(ro);
    drop(db);
    drop(storage);
    assert!(
        count_segments(dir.path()) >= 1,
        "compacted tip segment remains on disk"
    );
}

#[test]
fn readonly_does_not_see_writes_after_snapshot() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("iso");
    let store = setup_store(dir.path(), "iso");
    let storage = FsBackendFactory::new(dir.path())
        .open_storage(&key)
        .unwrap();
    let mut db = storage.open_database("db").unwrap();
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"x", b"old", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    let mut ro = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .unwrap();
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"x", b"new", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    assert_eq!(ro.get(store, b"x").unwrap(), Some(b"old".to_vec()));
}

#[test]
fn malformed_manifest_does_not_panic() {
    use boa_idb_fs::decode_frame; // touch crate
    let _ = decode_frame(&[]);
    // Exercise decode_manifest via open of crafted CURRENT.
    let dir = tempdir().unwrap();
    let key = StorageKey::new("bad-man");
    let storage = FsBackendFactory::new(dir.path())
        .open_storage(&key)
        .unwrap();
    let _ = storage.open_database("db").unwrap();
    drop(storage);

    // Overwrite CURRENT to point at garbage manifest — open falls back / errors safely.
    for sk in fs::read_dir(dir.path()).unwrap().flatten() {
        for db in fs::read_dir(sk.path()).unwrap().flatten() {
            let man = db.path().join("MANIFEST-000001");
            fs::write(&man, b"not-a-manifest").unwrap();
        }
    }
    let storage = FsBackendFactory::new(dir.path())
        .open_storage(&key)
        .unwrap();
    // Should not panic (either recovers with fallback or returns Err).
    let _ = storage.open_database("db");
}
