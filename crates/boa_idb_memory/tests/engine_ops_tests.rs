//! Engine-level tests: `ops_store` index maintenance, key-generator
//! persistence, savepoint nesting, and `*unique` cursor semantics — all
//! executed against the real in-memory backend.

#![allow(clippy::float_cmp)]

use boa_idb_core::backend::traits::{BackendTxn, Database};
use boa_idb_core::backend::types::{IndexMeta, StoreMeta};
use boa_idb_core::clone::scvalue::ScValue;
use boa_idb_core::engine::keygen::KeyGenerator;
use boa_idb_core::engine::ops_store;
use boa_idb_core::error::IdbError;
use boa_idb_core::key::encode::encode_key;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use boa_idb_core::proto::{Direction, Durability, SourceRef, StorageKey, TxnMode};
use indexmap::IndexMap;

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_memory::MemoryBackendFactory;

/// One scanned cursor row: (key, primary key, value).
type ScanRow = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);

fn limits() -> LimitConfig {
    LimitConfig::default()
}

fn open_db() -> Box<dyn Database> {
    let factory = MemoryBackendFactory::new();
    let storage = factory
        .open_storage(&StorageKey::new("engine-ops"))
        .unwrap();
    storage.open_database("testdb").unwrap()
}

fn begin_rw(db: &mut Box<dyn Database>) -> Box<dyn BackendTxn + 'static> {
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();
    txn.begin_request().unwrap();
    txn
}

fn index_meta(id: u64, name: &str, path: &str, unique: bool, multi_entry: bool) -> IndexMeta {
    IndexMeta {
        id,
        store_id: 1,
        name: name.into(),
        key_path: KeyPath::parse_single(path).unwrap(),
        unique,
        multi_entry,
        deleted: false,
    }
}

fn store_meta(indexes: Vec<IndexMeta>, auto_increment: bool) -> StoreMeta {
    StoreMeta {
        id: 1,
        name: "books".into(),
        key_path: KeyPath::parse_single("id").unwrap(),
        auto_increment,
        key_gen: 1.0,
        indexes,
        deleted: false,
    }
}

fn obj(pairs: &[(&str, ScValue)]) -> ScValue {
    let mut map = IndexMap::new();
    for (k, v) in pairs {
        map.insert(Utf16String::from(*k), v.clone());
    }
    ScValue::Object(map)
}

fn str_val(s: &str) -> ScValue {
    ScValue::String(s.into())
}

fn encode_str(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    encode_key(&Key::String(s.into()), &mut out, &limits()).unwrap();
    out
}

fn engine_put(
    txn: &mut dyn BackendTxn,
    meta: &StoreMeta,
    keygen: &mut KeyGenerator,
    value: &mut ScValue,
    no_overwrite: bool,
) -> Result<Key, IdbError> {
    ops_store::put(txn, meta, keygen, value, None, no_overwrite, &limits()).map(|r| r.key)
}

fn collect_scan(
    txn: &mut dyn BackendTxn,
    src: SourceRef,
    range: &EncodedRange,
    dir: Direction,
) -> Vec<ScanRow> {
    let mut cursor = txn.scan(src, range, dir, false).unwrap();
    let mut out = Vec::new();
    let mut active = cursor
        .seek(boa_idb_core::backend::types::CursorSeek::First)
        .unwrap();
    while active {
        out.push((
            cursor.current_key().to_vec(),
            cursor.current_primary_key().to_vec(),
            cursor.current_value().map(<[u8]>::to_vec),
        ));
        active = cursor.step(1).unwrap();
    }
    out
}

fn index_keys(txn: &mut dyn BackendTxn, index: u64, dir: Direction) -> Vec<Vec<u8>> {
    collect_scan(
        txn,
        SourceRef::Index { store: 1, index },
        &EncodedRange::all(),
        dir,
    )
    .into_iter()
    .map(|(k, _, _)| k)
    .collect()
}

// ===== D1: stale index entries removed on overwrite =====

#[test]
fn put_overwrite_removes_stale_index_entries() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(10, "by_tag", "tag", false, false)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0)), ("tag", str_val("a"))]),
        false,
    )
    .unwrap();
    engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0)), ("tag", str_val("b"))]),
        false,
    )
    .unwrap();

    assert_eq!(
        index_keys(&mut *txn, 10, Direction::Next),
        vec![encode_str("b")]
    );
    assert_eq!(
        txn.count(
            SourceRef::Index {
                store: 1,
                index: 10
            },
            &EncodedRange::all()
        )
        .unwrap(),
        1
    );
    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn put_self_overwrite_unique_without_change_is_ok() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(11, "by_v", "v", true, false)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    let v = &mut obj(&[("id", ScValue::Number(1.0)), ("v", str_val("x"))]);
    engine_put(&mut *txn, &meta, &mut keygen, v, false).unwrap();
    // Same record, same indexed keys: must not raise a false unique violation.
    let v2 = &mut obj(&[("id", ScValue::Number(1.0)), ("v", str_val("x"))]);
    engine_put(&mut *txn, &meta, &mut keygen, v2, false).unwrap();

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn put_unique_violation_across_records() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(11, "by_v", "v", true, false)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0)), ("v", str_val("x"))]),
        false,
    )
    .unwrap();
    let err = engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(2.0)), ("v", str_val("x"))]),
        false,
    )
    .unwrap_err();
    assert!(matches!(err, IdbError::Constraint(_)), "got {err:?}");
}

// ===== D9/D10: multiEntry dedup + Constraint mapping =====

#[test]
fn multientry_duplicates_collapsed() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(12, "by_tags", "tags", false, true)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    let tags = ScValue::Array {
        elements: vec![Some(str_val("x")), Some(str_val("x")), Some(str_val("y"))],
        extra_props: Vec::new(),
    };
    engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0)), ("tags", tags)]),
        false,
    )
    .unwrap();

    let x_range = EncodedRange::only(encode_str("x"));
    assert_eq!(
        txn.count(
            SourceRef::Index {
                store: 1,
                index: 12
            },
            &x_range
        )
        .unwrap(),
        1,
        "duplicate multiEntry values must collapse to one entry"
    );
    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn no_overwrite_violation_maps_to_constraint() {
    let mut db = open_db();
    let meta = store_meta(vec![], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0))]),
        true,
    )
    .unwrap();
    let err = engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0))]),
        true,
    )
    .unwrap_err();
    assert!(matches!(err, IdbError::Constraint(_)), "got {err:?}");
}

// ===== D1: delete/clear clean indexes =====

#[test]
fn delete_range_cleans_index_entries() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(10, "by_tag", "tag", false, false)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    for i in [1.0, 2.0] {
        engine_put(
            &mut *txn,
            &meta,
            &mut keygen,
            &mut obj(&[("id", ScValue::Number(i)), ("tag", str_val("t"))]),
            false,
        )
        .unwrap();
    }
    let deleted = ops_store::delete(&mut *txn, &meta, &EncodedRange::all(), &limits()).unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(
        txn.count(
            SourceRef::Index {
                store: 1,
                index: 10
            },
            &EncodedRange::all()
        )
        .unwrap(),
        0,
        "index entries must be removed together with the records"
    );
    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

#[test]
fn clear_cleans_index_entries() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(10, "by_tag", "tag", false, false)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0)), ("tag", str_val("t"))]),
        false,
    )
    .unwrap();
    ops_store::clear(&mut *txn, &meta).unwrap();
    assert_eq!(
        txn.count(
            SourceRef::Index {
                store: 1,
                index: 10
            },
            &EncodedRange::all()
        )
        .unwrap(),
        0
    );
    assert_eq!(
        txn.count(SourceRef::Store(1), &EncodedRange::all())
            .unwrap(),
        0
    );
    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

// ===== D3: key generator persistence =====

#[test]
fn keygen_value_persisted_and_rolled_back() {
    let mut db = open_db();
    let meta = store_meta(vec![], true);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    // No "id" in the value: auto-increment generates and injects it.
    let mut value = obj(&[("title", str_val("x"))]);
    let key = engine_put(&mut *txn, &meta, &mut keygen, &mut value, false).unwrap();
    assert_eq!(key, Key::Number(1.0));
    assert_eq!(
        value,
        obj(&[("title", str_val("x")), ("id", ScValue::Number(1.0)),]),
        "generated key must be injected into the stored value"
    );
    assert_eq!(txn.key_gen_current(1).unwrap(), 2.0);

    txn.rollback_request().unwrap();
    assert_eq!(txn.key_gen_current(1).unwrap(), 1.0);
}

// ===== D2/D13: nested savepoints + strict ordering =====

#[test]
fn nested_commit_merges_into_parent() {
    let mut db = open_db();
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();
    txn.put(1, b"a", b"1", false).unwrap();
    txn.begin_request().unwrap();
    txn.put(1, b"b", b"2", false).unwrap();
    txn.commit_request().unwrap();
    // Rolling back the outer level must undo BOTH levels.
    txn.rollback_request().unwrap();

    assert_eq!(txn.get(1, b"a").unwrap(), None);
    assert_eq!(txn.get(1, b"b").unwrap(), None);
}

#[test]
fn unbalanced_savepoint_ops_fail() {
    let mut db = open_db();
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();
    assert!(txn.commit_request().is_err());
    assert!(txn.rollback_request().is_err());
}

// ===== D4: *unique cursor dedup =====

#[test]
fn unique_cursors_yield_first_per_group() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(10, "by_tag", "tag", false, false)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    // Two records share the tag; pkeys 1.0 < 2.0.
    for i in [2.0, 1.0] {
        engine_put(
            &mut *txn,
            &meta,
            &mut keygen,
            &mut obj(&[("id", ScValue::Number(i)), ("tag", str_val("t"))]),
            false,
        )
        .unwrap();
    }
    let src = SourceRef::Index {
        store: 1,
        index: 10,
    };

    let next: Vec<Vec<u8>> = index_keys(&mut *txn, 10, Direction::Next);
    assert_eq!(next.len(), 2);

    let uniq: Vec<ScanRow> =
        collect_scan(&mut *txn, src, &EncodedRange::all(), Direction::NextUnique);
    assert_eq!(uniq.len(), 1, "nextunique collapses the group");
    // Representative is the smallest primary key.
    let mut pk = Vec::new();
    encode_key(&Key::Number(1.0), &mut pk, &limits()).unwrap();
    assert_eq!(uniq[0].1, pk);

    let prev_uniq = collect_scan(&mut *txn, src, &EncodedRange::all(), Direction::PrevUnique);
    assert_eq!(prev_uniq.len(), 1, "prevunique collapses the group");
    assert_eq!(prev_uniq[0].1, pk);

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

// ===== D5/D6: pending-visible index count/scan with values =====

#[test]
fn index_count_and_scan_see_pending() {
    let mut db = open_db();
    let meta = store_meta(vec![index_meta(10, "by_tag", "tag", false, false)], false);
    let mut keygen = KeyGenerator::new(1.0);
    let mut txn = begin_rw(&mut db);

    engine_put(
        &mut *txn,
        &meta,
        &mut keygen,
        &mut obj(&[("id", ScValue::Number(1.0)), ("tag", str_val("t"))]),
        false,
    )
    .unwrap();

    // Uncommitted, but visible inside the transaction (read-your-writes).
    let src = SourceRef::Index {
        store: 1,
        index: 10,
    };
    assert_eq!(txn.count(src, &EncodedRange::all()).unwrap(), 1);
    let rows = collect_scan(&mut *txn, src, &EncodedRange::all(), Direction::Next);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].2.is_some(), "index scan must resolve record values");

    txn.commit_request().unwrap();
    txn.commit().unwrap();
}

// ===== D7: pending-only delete_range with undo =====

#[test]
fn delete_range_pending_only_rolls_back() {
    let mut db = open_db();
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[1], Durability::Default)
        .unwrap();

    txn.begin_request().unwrap();
    txn.put(1, b"k", b"v", false).unwrap();
    txn.commit_request().unwrap();
    // The record is still pending (uncommitted to storage); delete it on a
    // new level, then roll that level back.
    txn.begin_request().unwrap();
    let deleted = txn.delete_range(1, &EncodedRange::all()).unwrap();
    assert_eq!(deleted, 1);
    txn.rollback_request().unwrap();

    assert_eq!(txn.get(1, b"k").unwrap(), Some(b"v".to_vec()));
}
