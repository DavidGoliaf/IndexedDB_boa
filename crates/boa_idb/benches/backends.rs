//! M7-A `criterion` benchmarks: TZ §12.1 scenarios for `SQLite` and FS.
//!
//! The `memory` backend runs as a control (no targets apply to it). Every
//! benchmark uses an isolated temp root, a fixed seed layout (numeric `u64`
//! big-endian keys, ~200 B values), and the stated durability. Fixture
//! setup/teardown is excluded from the measured section unless the scenario
//! explicitly measures open/close.
//!
//! Scale is normative by default, except the cursor scan: `cursor_scan_<N>`
//! runs at the diagnostic 20 000-record scale and the normative 1M full
//! scan lives in the `scan-1m` example runner (P1-2). Set
//! `BOA_IDB_BENCH_SMOKE=1` for a fast smoke run (CI); smoke and diagnostic
//! numbers are never release evidence. See `benches/receipts/README.md` for
//! the receipt/baseline workflow.
//!
//! Method notes (steady-state choices, all documented for reproducibility):
//!
//! * `put_batch` overwrites the same key range on every iteration after the
//!   first, so later iterations measure steady-state overwrite `put`
//!   throughput at constant DB size rather than unbounded growth.
//! * `put_batch` wraps every `put` in a request savepoint
//!   (`begin_request`/`commit_request`): that is what the production request
//!   path does per `IDBRequest`, so the benchmark stays faithful to it.
//! * `open` measures `open_database` + a metadata read; the matching `close`
//!   is included in the iteration (it is unflushed and negligible next to
//!   open/recovery — stated here so receipts stay honest).
//! * `js_request` measures one JS `get` round trip (`eval` + two job-queue
//!   drains) against the memory backend: pure JS↔core overhead, no IO. The
//!   per-iteration readback keeps the measurement honest.
//! * `open` is skipped for `memory` (no persistence/recovery there; the
//!   number would be meaningless).

#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, missing_docs)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use boa_engine::{Context, Source};
use boa_idb::extension::IndexedDbExtension;
use boa_idb_core::backend::traits::{BackendFactory, Database, Storage};
use boa_idb_core::backend::types::{CursorSeek, StoreSpec};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Direction, Durability, SourceRef, StorageKey, StoreId, TxnMode};
use boa_idb_fs::FsBackendFactory;
use boa_idb_memory::MemoryBackendFactory;
use boa_idb_sqlite::SqliteBackendFactory;
use criterion::{
    BenchmarkGroup, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use tempfile::TempDir;

/// Value size in bytes for every scenario (~200 B per §12.1).
const VALUE_LEN: usize = 200;

/// Diagnostic-scale record counts.
///
/// `SCAN_FILL` is 20 000: the diagnostic cursor-scan scale. The normative
/// 1M full scan runs through the `scan-1m` example runner (P1-2), which
/// reports a real result or a controlled timeout — a 1M `SQLite` scan does
/// not fit routine statistics (criterion estimated ~21 000 s for 20 samples
/// on 2026-09-05). Diagnostic rates stay comparable across backends at
/// equal `N`, but they are never normative evidence.
const PUT_BATCH: usize = 10_000;
const GET_BATCH: usize = 1_000;
const SCAN_FILL: usize = 20_000;
const OPEN_FILL: usize = 1_000_000;
const STRICT_BATCH: usize = 5;
const EMPTY_BATCH: usize = 50;

/// Smoke-scale record counts (`BOA_IDB_BENCH_SMOKE=1`).
const SMOKE_PUT_BATCH: usize = 200;
const SMOKE_GET_BATCH: usize = 100;
const SMOKE_SCAN_FILL: usize = 2_000;
const SMOKE_OPEN_FILL: usize = 20_000;
const SMOKE_STRICT_BATCH: usize = 2;
const SMOKE_EMPTY_BATCH: usize = 10;

/// Per-run scale knobs.
struct Scale {
    put_batch: usize,
    get_batch: usize,
    scan_fill: usize,
    open_fill: usize,
    strict_batch: usize,
    empty_batch: usize,
}

/// Returns the active scale; smoke mode is for CI speed, never evidence.
fn scale() -> Scale {
    if std::env::var("BOA_IDB_BENCH_SMOKE").is_ok() {
        Scale {
            put_batch: SMOKE_PUT_BATCH,
            get_batch: SMOKE_GET_BATCH,
            scan_fill: SMOKE_SCAN_FILL,
            open_fill: SMOKE_OPEN_FILL,
            strict_batch: SMOKE_STRICT_BATCH,
            empty_batch: SMOKE_EMPTY_BATCH,
        }
    } else {
        Scale {
            put_batch: PUT_BATCH,
            get_batch: GET_BATCH,
            scan_fill: SCAN_FILL,
            open_fill: OPEN_FILL,
            strict_batch: STRICT_BATCH,
            empty_batch: EMPTY_BATCH,
        }
    }
}

/// Backend under measurement.
#[derive(Debug, Clone, Copy)]
enum BackendKind {
    Memory,
    Sqlite,
    Fs,
}

impl BackendKind {
    /// Short label used in benchmark ids.
    fn label(self) -> &'static str {
        match self {
            BackendKind::Memory => "memory",
            BackendKind::Sqlite => "sqlite",
            BackendKind::Fs => "fs",
        }
    }

    /// Creates a factory rooted at `root` (ignored by `memory`).
    fn factory(self, root: &Path) -> Box<dyn BackendFactory> {
        match self {
            BackendKind::Memory => Box::new(MemoryBackendFactory::new()),
            BackendKind::Sqlite => Box::new(SqliteBackendFactory::new(root)),
            BackendKind::Fs => Box::new(FsBackendFactory::new(root)),
        }
    }

    /// Whether the open scenario applies (needs persistence/recovery).
    fn measures_open(self) -> bool {
        !matches!(self, BackendKind::Memory)
    }
}

/// Encodes a numeric key as fixed-width big-endian bytes (ordered).
fn key_bytes(i: u64) -> [u8; 8] {
    i.to_be_bytes()
}

/// Builds a deterministic ~200 B value.
fn value_bytes(seed: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(VALUE_LEN);
    for i in 0..VALUE_LEN {
        v.push((seed.wrapping_add(i as u64) & 0xff) as u8);
    }
    v
}

/// Opens (or creates) the bench database and ensures store `s` exists.
///
/// Each scenario group uses its own database name so fixtures sharing one
/// temp root never contaminate each other's record counts (P1-2 follow-up
/// fix: the shared `benchdb` made scan asserts fail at smoke scale).
fn open_bench_db(storage: &dyn Storage, db_name: &str) -> (Box<dyn Database>, StoreId) {
    let mut db = storage.open_database(db_name).expect("open database");
    let existing = db
        .metadata()
        .stores
        .iter()
        .find(|s| s.name.to_string() == "s")
        .map(|s| s.id);
    let store = if let Some(id) = existing {
        id
    } else {
        let mut txn = db
            .begin(TxnMode::VersionChange, &[], Durability::Default)
            .expect("begin versionchange");
        txn.set_version(1).expect("set version");
        let id = txn
            .create_store(&StoreSpec {
                name: Utf16String::from("s"),
                key_path: KeyPath::Empty,
                auto_increment: false,
            })
            .expect("create store");
        txn.commit().expect("commit versionchange");
        id
    };
    (db, store)
}

/// Fills the bench database with `fill` records in 10 000-record chunks.
///
/// Fill commits use `Relaxed` durability so fixture construction itself
/// stays fast; the durability under test is set per scenario, not here.
fn setup_fixture(kind: BackendKind, tmp: &TempDir, fill: usize, db_name: &str) -> Box<dyn Storage> {
    let factory = kind.factory(tmp.path());
    let storage = factory
        .open_storage(&StorageKey::new("bench"))
        .expect("open storage");
    let (mut db, store) = open_bench_db(&*storage, db_name);
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
        .expect("begin fill");
    for (n, i) in (0..(fill as u64)).enumerate() {
        txn.begin_request().expect("begin request");
        txn.put(store, &key_bytes(i), &value_bytes(i), false)
            .expect("fill put");
        txn.commit_request().expect("commit request");
        if (n + 1) % PUT_BATCH == 0 {
            txn.commit().expect("fill chunk commit");
            txn = db
                .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
                .expect("begin fill chunk");
        }
    }
    txn.commit().expect("fill commit");
    db.close().expect("close after fill");
    storage
}

/// Reports `n` elements for the benchmark just registered on `group`.
fn elements(group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>, n: usize) {
    group.throughput(Throughput::Elements(u64::try_from(n).unwrap_or(u64::MAX)));
}

/// `put` batch: one `Relaxed` readwrite txn with `n` savepointed puts.
fn bench_put_batch(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    storage: &dyn Storage,
    db_name: &str,
    n: usize,
) {
    let (_, store) = open_bench_db(storage, db_name);
    let mut db = storage.open_database(db_name).expect("open");
    elements(group, n);
    group.bench_function("put_batch", |b| {
        b.iter(|| {
            let mut txn = db
                .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
                .expect("begin");
            for i in 0..(n as u64) {
                txn.begin_request().expect("begin request");
                txn.put(store, &key_bytes(i), &value_bytes(i), false)
                    .expect("put");
                txn.commit_request().expect("commit request");
            }
            txn.commit().expect("commit");
        });
    });
    db.close().expect("close bench db");
}

/// Primary-key `get` in a single readonly txn, `n` gets per iteration.
fn bench_pk_get(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    storage: &dyn Storage,
    db_name: &str,
    fill: usize,
    n: usize,
) {
    let (_, store) = open_bench_db(storage, db_name);
    let mut db = storage.open_database(db_name).expect("open");
    elements(group, n);
    group.bench_function("pk_get", |b| {
        b.iter(|| {
            let mut txn = db
                .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
                .expect("begin");
            let mut hits = 0_u64;
            for i in 0..(n as u64) {
                let key = key_bytes(i % (fill as u64));
                if txn.get(store, &key).expect("get").is_some() {
                    hits += 1;
                }
            }
            txn.commit().expect("commit readonly");
            black_box(hits);
        });
    });
    db.close().expect("close bench db");
}

/// Diagnostic cursor scan of `fill` records; asserts the full range visited.
///
/// This is explicitly NOT the normative scenario: the normative full scan
/// runs at 1 000 000 records through the `scan-1m` example runner (P1-2),
/// because a 1M scan does not fit routine statistics on every backend. The
/// benchmark id carries the record count (`cursor_scan_<N>`) so diagnostic
/// numbers can never be mistaken for normative evidence.
fn bench_cursor_scan(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    storage: &dyn Storage,
    db_name: &str,
    fill: usize,
) {
    let (_, store) = open_bench_db(storage, db_name);
    let mut db = storage.open_database(db_name).expect("open");
    elements(group, fill);
    group.bench_function(format!("cursor_scan_{fill}"), |b| {
        b.iter(|| {
            let mut txn = db
                .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
                .expect("begin");
            let count = {
                let mut cursor = txn
                    .scan(
                        SourceRef::Store(store),
                        &EncodedRange::all(),
                        Direction::Next,
                        false,
                    )
                    .expect("scan");
                assert!(cursor.seek(CursorSeek::First).expect("seek"));
                let mut count = 1_usize;
                while cursor.step(1).expect("step") {
                    count += 1;
                    black_box(cursor.current_value());
                }
                count
            };
            assert_eq!(count, fill, "full range must be visited");
            txn.commit().expect("commit readonly");
        });
    });
    db.close().expect("close bench db");
}

/// Empty readwrite transactions (create + commit), `n` per iteration.
fn bench_empty_txn(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    storage: &dyn Storage,
    db_name: &str,
    n: usize,
) {
    let (_, store) = open_bench_db(storage, db_name);
    let mut db = storage.open_database(db_name).expect("open");
    elements(group, n);
    group.bench_function("empty_txn", |b| {
        b.iter(|| {
            for _ in 0..n {
                let txn = db
                    .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
                    .expect("begin");
                txn.commit().expect("commit");
            }
        });
    });
    db.close().expect("close bench db");
}

/// Single strict commits, `n` per iteration with unique keys.
fn bench_strict_commit(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    storage: &dyn Storage,
    db_name: &str,
    fill: usize,
    n: usize,
) {
    let (_, store) = open_bench_db(storage, db_name);
    let mut db = storage.open_database(db_name).expect("open");
    let mut seq = fill as u64;
    elements(group, n);
    group.bench_function("strict_commit", |b| {
        b.iter(|| {
            for _ in 0..n {
                let mut txn = db
                    .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
                    .expect("begin");
                txn.begin_request().expect("begin request");
                txn.put(store, &key_bytes(seq), &value_bytes(seq), false)
                    .expect("put");
                txn.commit_request().expect("commit request");
                txn.commit().expect("strict commit");
                seq += 1;
            }
        });
    });
    db.close().expect("close bench db");
    black_box(seq);
}

/// Open of a database holding `fill` records (persistence backends only).
fn bench_open(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    storage: &dyn Storage,
    db_name: &str,
) {
    group.bench_function("open", |b| {
        b.iter(|| {
            let db = storage.open_database(db_name).expect("open");
            black_box(db.metadata().stores.len());
            db.close().expect("close");
        });
    });
}

/// One JS `get` round trip without IO (memory backend): eval + job drains.
fn bench_js_request(c: &mut Criterion) {
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("bench-js"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .build()
        .expect("build extension");
    extension
        .register(&mut context)
        .expect("register extension");
    context
        .eval(Source::from_bytes(
            r"
            globalThis.__benchN = 0;
            let openReq = indexedDB.open('jsbench', 1);
            openReq.onupgradeneeded = () => {
                openReq.result.createObjectStore('s', { keyPath: 'id' });
            };
            openReq.onsuccess = () => {
                globalThis.__benchDb = openReq.result;
                const tx = globalThis.__benchDb.transaction('s', 'readwrite');
                tx.objectStore('s').put({ id: 1, v: 'x'.repeat(200) });
            };
            ",
        ))
        .expect("setup eval");
    context.run_jobs().expect("setup jobs");
    context.run_jobs().expect("setup jobs");
    // Validate the fixture before measuring.
    context
        .eval(Source::from_bytes(
            r"
            const tx = globalThis.__benchDb.transaction('s', 'readonly');
            const rq = tx.objectStore('s').get(1);
            rq.onsuccess = () => { globalThis.__benchN += 1; };
            ",
        ))
        .expect("validation eval");
    context.run_jobs().expect("validation jobs");
    context.run_jobs().expect("validation jobs");
    let n: String = context
        .eval(Source::from_bytes("String(globalThis.__benchN)"))
        .expect("readback")
        .to_string(&mut context)
        .expect("to_string")
        .to_std_string_escaped();
    assert!(n.contains('1'), "fixture get must complete, n={n}");

    let mut group = c.benchmark_group("core");
    group.bench_function("js_request", |b| {
        b.iter(|| {
            // Block-scoped so repeated evals never redeclare globals.
            context
                .eval(Source::from_bytes(
                    r"{
                    const tx = globalThis.__benchDb.transaction('s', 'readonly');
                    const rq = tx.objectStore('s').get(1);
                    rq.onsuccess = () => { globalThis.__benchN += 1; };
                    }",
                ))
                .expect("iter eval");
            context.run_jobs().expect("iter jobs");
            context.run_jobs().expect("iter jobs");
            let n: String = context
                .eval(Source::from_bytes("String(globalThis.__benchN)"))
                .expect("iter readback")
                .to_string(&mut context)
                .expect("to_string")
                .to_std_string_escaped();
            black_box(n);
        });
    });
    group.finish();
}

/// All §12.1 scenarios per backend (memory runs as control, without `open`).
fn backend_benches(c: &mut Criterion) {
    let s = scale();
    for kind in [BackendKind::Memory, BackendKind::Sqlite, BackendKind::Fs] {
        // Keep each backend on its own temp root (isolation per §3.1), with
        // one fixture per scenario scale: sharing a single 1M fixture made
        // the SQLite cursor scan unmeasurable (see `SCAN_FILL`).
        let tmp = tempfile::tempdir().expect("tempdir");
        let crud_storage = setup_fixture(kind, &tmp, s.put_batch, "bench_crud");
        let mut group = c.benchmark_group(kind.label());
        bench_put_batch(&mut group, &*crud_storage, "bench_crud", s.put_batch);
        bench_pk_get(
            &mut group,
            &*crud_storage,
            "bench_crud",
            s.put_batch,
            s.get_batch,
        );
        bench_empty_txn(&mut group, &*crud_storage, "bench_crud", s.empty_batch);
        bench_strict_commit(
            &mut group,
            &*crud_storage,
            "bench_crud",
            s.put_batch,
            s.strict_batch,
        );
        group.finish();

        let scan_storage = setup_fixture(kind, &tmp, s.scan_fill, "bench_scan");
        let mut scan_group = c.benchmark_group(kind.label());
        bench_cursor_scan(&mut scan_group, &*scan_storage, "bench_scan", s.scan_fill);
        scan_group.finish();

        if kind.measures_open() {
            let open_storage = setup_fixture(kind, &tmp, s.open_fill, "bench_open");
            // Reopen the backend group so the id reads `<backend>/open`.
            let mut open_group = c.benchmark_group(kind.label());
            open_group.sample_size(10);
            open_group.measurement_time(Duration::from_secs(5));
            bench_open(&mut open_group, &*open_storage, "bench_open");
            open_group.finish();
        }
    }
    bench_js_request(c);
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(3))
        .warm_up_time(Duration::from_secs(1));
    targets = backend_benches
);
criterion_main!(benches);
