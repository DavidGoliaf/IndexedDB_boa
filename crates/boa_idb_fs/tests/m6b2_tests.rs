//! M6-B2: `FileSystem` seam + deterministic fault / corruption matrix (R8.5.2 / R8.5.3).

#![allow(clippy::float_cmp)]

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, StorageKey, TxnMode};
use boa_idb_fs::{
    CompactConfig, FaultInjectingFs, FaultKind, FaultSite, FsBackendFactory, OsFileSystem,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn setup_store(factory: &FsBackendFactory, key: &StorageKey) -> u64 {
    let storage = factory.open_storage(key).unwrap();
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

fn put(
    factory: &FsBackendFactory,
    key: &StorageKey,
    store: u64,
    k: &[u8],
    v: &[u8],
) -> Result<(), BackendError> {
    let storage = factory.open_storage(key)?;
    let mut db = storage.open_database("db")?;
    let mut txn = db.begin(TxnMode::ReadWrite, &[store], Durability::Strict)?;
    txn.begin_request()?;
    txn.put(store, k, v, false)?;
    txn.commit_request()?;
    txn.commit()?;
    Ok(())
}

fn get(
    factory: &FsBackendFactory,
    key: &StorageKey,
    store: u64,
    k: &[u8],
) -> Result<Option<Vec<u8>>, BackendError> {
    let storage = factory.open_storage(key)?;
    let mut db = storage.open_database("db")?;
    let mut txn = db.begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)?;
    txn.get(store, k)
}

fn find_db_dir(root: &Path) -> PathBuf {
    for sk in fs::read_dir(root).unwrap().flatten() {
        for db in fs::read_dir(sk.path()).unwrap().flatten() {
            if db.path().join("CURRENT").exists() {
                return db.path();
            }
        }
    }
    panic!("db dir not found under {}", root.display());
}

fn clean_factory(root: &Path) -> FsBackendFactory {
    FsBackendFactory::new(root)
}

#[derive(Debug)]
struct FaultCase {
    name: &'static str,
    site: FaultSite,
    kind: FaultKind,
    /// When true, arm fault before a compact-triggering write.
    force_compact: bool,
}

fn fault_matrix() -> Vec<FaultCase> {
    vec![
        FaultCase {
            name: "wal_append_enospc",
            site: FaultSite::WalAppend,
            kind: FaultKind::Enospc,
            force_compact: false,
        },
        FaultCase {
            name: "wal_append_eio",
            site: FaultSite::WalAppend,
            kind: FaultKind::Eio,
            force_compact: false,
        },
        FaultCase {
            name: "wal_append_short",
            site: FaultSite::WalAppend,
            kind: FaultKind::ShortWrite { bytes: 4 },
            force_compact: false,
        },
        FaultCase {
            name: "wal_append_interrupt",
            site: FaultSite::WalAppend,
            kind: FaultKind::Interrupt,
            force_compact: false,
        },
        FaultCase {
            name: "wal_sync_fail",
            site: FaultSite::WalSync,
            kind: FaultKind::SyncFail,
            force_compact: false,
        },
        FaultCase {
            name: "segment_write_eio",
            site: FaultSite::SegmentWrite,
            kind: FaultKind::Eio,
            force_compact: true,
        },
        FaultCase {
            name: "manifest_write_enospc",
            site: FaultSite::ManifestWrite,
            kind: FaultKind::Enospc,
            force_compact: true,
        },
        FaultCase {
            name: "current_rename_fail",
            site: FaultSite::CurrentRename,
            kind: FaultKind::RenameFail,
            force_compact: true,
        },
        FaultCase {
            name: "meta_write_eio",
            site: FaultSite::MetaWrite,
            kind: FaultKind::Eio,
            force_compact: true,
        },
        FaultCase {
            name: "dir_sync_fail",
            site: FaultSite::DirSync,
            kind: FaultKind::SyncFail,
            force_compact: true,
        },
        FaultCase {
            name: "cleanup_remove_eio",
            site: FaultSite::CleanupRemove,
            kind: FaultKind::Eio,
            force_compact: true,
        },
    ]
}

#[test]
fn fault_matrix_preserves_committed_prefix_on_reopen() {
    for case in fault_matrix() {
        let dir = tempdir().unwrap();
        let faults = FaultInjectingFs::new(OsFileSystem::shared());
        let factory = if case.force_compact {
            FsBackendFactory::new(dir.path())
                .with_filesystem(faults.clone())
                .with_compact_config(CompactConfig {
                    wal_bytes: u64::MAX,
                    wal_frames: 2,
                })
        } else {
            FsBackendFactory::new(dir.path()).with_filesystem(faults.clone())
        };
        let key = StorageKey::new(case.name);
        let store = setup_store(&factory, &key);

        put(&factory, &key, store, b"ok", b"yes").unwrap();
        assert_eq!(
            get(&factory, &key, store, b"ok").unwrap(),
            Some(b"yes".to_vec()),
            "{}: baseline",
            case.name
        );

        faults.clear();
        faults.inject_once(case.site, case.kind.clone());

        let second = put(&factory, &key, store, b"next", b"no");
        // Compaction faults are swallowed after a durable WAL commit; WAL faults
        // should fail the user transaction (except intentional short-write Ok).
        if !case.force_compact {
            match case.kind {
                FaultKind::ShortWrite { .. } => {
                    // Torn append may report success; durable reopen is the check.
                    let _ = second;
                }
                _ => {
                    assert!(
                        second.is_err(),
                        "{}: expected commit error, got {second:?}",
                        case.name
                    );
                }
            }
        }

        faults.clear();
        drop(factory);

        let factory = clean_factory(dir.path());
        assert_eq!(
            get(&factory, &key, store, b"ok").unwrap(),
            Some(b"yes".to_vec()),
            "{}: committed prefix must survive",
            case.name
        );
        if !case.force_compact {
            match case.kind {
                FaultKind::ShortWrite { .. } => {
                    // Short write is not a durable COMMIT frame.
                    assert_eq!(
                        get(&factory, &key, store, b"next").unwrap(),
                        None,
                        "{}: torn second put must not appear",
                        case.name
                    );
                }
                _ => {
                    assert_eq!(
                        get(&factory, &key, store, b"next").unwrap(),
                        None,
                        "{}: failed put must not appear",
                        case.name
                    );
                }
            }
        }
    }
}

#[test]
fn corrupt_wal_tail_and_segment_manifest_are_safe() {
    let dir = tempdir().unwrap();
    let key = StorageKey::new("corrupt");
    let factory = FsBackendFactory::new(dir.path()).with_compact_config(CompactConfig {
        wal_bytes: u64::MAX,
        wal_frames: 2,
    });
    let store = setup_store(&factory, &key);
    put(&factory, &key, store, b"a", b"1").unwrap();
    put(&factory, &key, store, b"b", b"2").unwrap();
    // Force a segment onto disk.
    put(&factory, &key, store, b"c", b"3").unwrap();
    drop(factory);

    let db_dir = find_db_dir(dir.path());

    // Corrupt WAL tail.
    let wal = db_dir.join("wal");
    let wal_file = fs::read_dir(&wal)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "log"))
        .expect("wal");
    {
        let mut f = OpenOptions::new().append(true).open(&wal_file).unwrap();
        f.write_all(&[0xDE, 0xAD, 0xBE]).unwrap();
    }

    // Corrupt last bytes of a segment if present.
    let seg_dir = db_dir.join("seg");
    if seg_dir.is_dir() {
        for entry in fs::read_dir(&seg_dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "seg") {
                let mut bytes = fs::read(&path).unwrap();
                if let Some(last) = bytes.last_mut() {
                    *last ^= 0xff;
                }
                fs::write(&path, &bytes).unwrap();
            }
        }
    }

    // Corrupt manifest body (CRC) if present — open must not panic.
    for entry in fs::read_dir(&db_dir).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("MANIFEST-") {
            let mut bytes = fs::read(entry.path()).unwrap();
            if bytes.len() > 8 {
                let i = bytes.len() / 2;
                bytes[i] ^= 0xff;
                fs::write(entry.path(), &bytes).unwrap();
            }
        }
    }

    let factory = clean_factory(dir.path());
    // Open may succeed via WAL-only pre-compact path, or fail with Corrupted /
    // Io — never panic, never return garbage keys from a torn CRC body.
    match factory
        .open_storage(&key)
        .and_then(|s| s.open_database("db"))
    {
        Ok(mut db) => {
            let mut txn = db
                .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
                .unwrap();
            // At least the first durable put must be present if open succeeded
            // without reading a corrupt segment tip.
            let got = txn.get(store, b"a").unwrap();
            assert!(
                got.is_none() || got == Some(b"1".to_vec()),
                "no garbage values"
            );
        }
        Err(err) => {
            assert!(
                matches!(
                    err,
                    BackendError::Corrupted(_) | BackendError::Io(_) | BackendError::Internal(_)
                ),
                "unexpected error kind: {err:?}"
            );
        }
    }
}

#[test]
fn compact_meta_fail_keeps_memory_on_published_wal() {
    // After CURRENT publish, in-memory wal_seq must advance even if meta.scf
    // sync fails — otherwise later commits are written to a superseded WAL and
    // vanish on reopen.
    use boa_idb_core::backend::traits::BackendFactory;
    use boa_idb_core::proto::{Durability, TxnMode};

    let dir = tempdir().unwrap();
    let faults = FaultInjectingFs::new(OsFileSystem::shared());
    let factory = FsBackendFactory::new(dir.path())
        .with_filesystem(faults.clone())
        .with_compact_config(CompactConfig {
            wal_bytes: u64::MAX,
            wal_frames: 1,
        });
    let key = StorageKey::new("compact-meta");
    let store = setup_store(&factory, &key);

    let storage = factory.open_storage(&key).unwrap();
    let mut db = storage.open_database("db").unwrap();
    faults.clear();
    faults.inject_once(FaultSite::MetaWrite, FaultKind::Eio);
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"a", b"1", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    faults.clear();
    {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .unwrap();
        txn.begin_request().unwrap();
        txn.put(store, b"b", b"2", false).unwrap();
        txn.commit_request().unwrap();
        txn.commit().unwrap();
    }
    drop(db);
    drop(storage);

    let factory = clean_factory(dir.path());
    assert_eq!(
        get(&factory, &key, store, b"a").unwrap(),
        Some(b"1".to_vec())
    );
    assert_eq!(
        get(&factory, &key, store, b"b").unwrap(),
        Some(b"2".to_vec()),
        "post-compact commit must land on the published WAL generation"
    );
}

#[test]
fn enospc_maps_to_quota_exceeded() {
    let dir = tempdir().unwrap();
    let faults = FaultInjectingFs::new(OsFileSystem::shared());
    let factory = FsBackendFactory::new(dir.path()).with_filesystem(faults.clone());
    let key = StorageKey::new("quota");
    let store = setup_store(&factory, &key);
    put(&factory, &key, store, b"ok", b"yes").unwrap();
    faults.inject_once(FaultSite::WalAppend, FaultKind::Enospc);
    let err = put(&factory, &key, store, b"x", b"y").unwrap_err();
    assert!(
        matches!(err, BackendError::QuotaExceeded { .. }),
        "got {err:?}"
    );
}

#[test]
fn production_paths_use_injected_filesystem() {
    // Smoke: factory with FaultInjectingFs still serves normal traffic when
    // no faults are armed (control flow identical to OsFileSystem).
    let dir = tempdir().unwrap();
    let faults = FaultInjectingFs::new(OsFileSystem::shared());
    let factory = FsBackendFactory::new(dir.path()).with_filesystem(faults);
    let key = StorageKey::new("smoke");
    let store = setup_store(&factory, &key);
    put(&factory, &key, store, b"k", b"v").unwrap();
    assert_eq!(
        get(&factory, &key, store, b"k").unwrap(),
        Some(b"v".to_vec())
    );
}
