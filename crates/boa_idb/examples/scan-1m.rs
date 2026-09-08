//! Normative 1M full cursor scan runner (M7-A rework P1-2, §12.1).
//!
//! The default `criterion` suite measures cursor scans at a diagnostic
//! 20 000-record scale (`cursor_scan_<N>` ids) because a 1M scan does not
//! fit routine benchmark statistics on every backend. **This runner is the
//! normative 1M scenario**: one full scan pass over `records` (default
//! 1 000 000) with a controlled wall-clock timeout.
//!
//! Verdict semantics (see the M7-A receipt for the recorded instance):
//!
//! * `RESULT` + `records_per_s` meeting the §12.1 target ⇒ `PASS`;
//! * `RESULT` below target, or `TIMEOUT` before the full range is visited ⇒
//!   `MISS` (never re-labeled, never deleted; profiling is M7-B).
//!
//! Fixture layout matches the `criterion` suite (numeric `u64` big-endian
//! keys, ~200 B values, store `s`) so rates stay comparable. Fixture
//! construction is setup (excluded from the measured scan) but its wall
//! time is reported. Flags:
//!
//! ```sh
//! cargo run -p boa_idb --example scan-1m -- \
//!   --backend sqlite --root ./scan-1m-data [--records 1000000] \
//!   [--timeout-secs 1800] [--storage-key scan-1m] [--reuse] [--build-only]
//! ```
//!
//! Exit codes: `0` result (pass or miss — the verdict is in the output),
//! `1` usage error, `2` backend error, `3` controlled scan timeout.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::CursorSeek;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, Durability, SourceRef, StorageKey, TxnMode};

/// Default record count (normative scale).
const DEFAULT_RECORDS: u64 = 1_000_000;
/// Default scan timeout (seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 1_800;
/// Fixture commit chunk (matches the `criterion` suite).
const FILL_CHUNK: u64 = 10_000;
/// Timeout check granularity (rows).
const CHECK_EVERY: u64 = 8_192;
/// Value size in bytes (~200 B per §12.1).
const VALUE_LEN: usize = 200;

/// Runner failures with process exit codes.
#[derive(Debug, thiserror::Error)]
enum ScanError {
    /// Command-line usage error.
    #[error("usage: {0}")]
    Usage(String),
    /// Backend operation failed.
    #[error("backend error: {0}")]
    Backend(String),
}

impl ScanError {
    /// Process exit code for this failure.
    fn code(&self) -> i32 {
        match self {
            ScanError::Usage(_) => 1,
            ScanError::Backend(_) => 2,
        }
    }
}

impl From<BackendError> for ScanError {
    fn from(error: BackendError) -> Self {
        ScanError::Backend(error.to_string())
    }
}

/// Parsed command line.
struct Args {
    backend: String,
    root: PathBuf,
    records: u64,
    timeout: Duration,
    storage_key: String,
    reuse: bool,
    build_only: bool,
}

/// Parses `std::env::args` (hand-rolled: no argument-parser dependency).
fn parse_args(argv: &[String]) -> Result<Args, ScanError> {
    let usage = "scan-1m --backend <sqlite|fs|memory> --root <dir> [--records N] [--timeout-secs T] [--storage-key K] [--reuse] [--build-only]";
    let mut backend: Option<String> = None;
    let mut root: Option<PathBuf> = None;
    let mut records = DEFAULT_RECORDS;
    let mut timeout_secs = DEFAULT_TIMEOUT_SECS;
    let mut storage_key = String::from("scan-1m");
    let mut reuse = false;
    let mut build_only = false;
    let mut iter = argv.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--backend" => {
                backend = Some(
                    iter.next()
                        .cloned()
                        .ok_or_else(|| ScanError::Usage(usage.into()))?,
                );
            }
            "--root" => {
                root = Some(PathBuf::from(
                    iter.next().ok_or_else(|| ScanError::Usage(usage.into()))?,
                ));
            }
            "--records" => {
                let raw = iter.next().ok_or_else(|| ScanError::Usage(usage.into()))?;
                records = raw
                    .parse::<u64>()
                    .map_err(|_| ScanError::Usage(format!("invalid --records '{raw}'")))?;
                if records == 0 {
                    return Err(ScanError::Usage("--records must be positive".into()));
                }
            }
            "--timeout-secs" => {
                let raw = iter.next().ok_or_else(|| ScanError::Usage(usage.into()))?;
                timeout_secs = raw
                    .parse::<u64>()
                    .map_err(|_| ScanError::Usage(format!("invalid --timeout-secs '{raw}'")))?;
                if timeout_secs == 0 {
                    return Err(ScanError::Usage("--timeout-secs must be positive".into()));
                }
            }
            "--storage-key" => {
                storage_key = iter
                    .next()
                    .cloned()
                    .ok_or_else(|| ScanError::Usage(usage.into()))?;
            }
            "--reuse" => reuse = true,
            "--build-only" => build_only = true,
            "--help" | "-h" => return Err(ScanError::Usage(usage.into())),
            other => return Err(ScanError::Usage(format!("unexpected argument '{other}'"))),
        }
    }
    let backend = backend.ok_or_else(|| ScanError::Usage(usage.into()))?;
    if backend != "sqlite" && backend != "fs" && backend != "memory" {
        return Err(ScanError::Usage(format!(
            "unknown backend '{backend}' (expected sqlite|fs|memory)"
        )));
    }
    let root = root.ok_or_else(|| ScanError::Usage(usage.into()))?;
    Ok(Args {
        backend,
        root,
        records,
        timeout: Duration::from_secs(timeout_secs),
        storage_key,
        reuse,
        build_only,
    })
}

/// Opens storage for the requested backend rooted at `root`.
fn open_storage(args: &Args) -> Result<Box<dyn boa_idb_core::backend::traits::Storage>, ScanError> {
    let key = StorageKey::new(args.storage_key.clone());
    match args.backend.as_str() {
        "sqlite" => {
            let factory = boa_idb_sqlite::SqliteBackendFactory::new(&args.root);
            factory.open_storage(&key).map_err(ScanError::from)
        }
        "fs" => {
            let factory = boa_idb_fs::FsBackendFactory::new(&args.root);
            factory.open_storage(&key).map_err(ScanError::from)
        }
        "memory" => {
            let factory = boa_idb_memory::MemoryBackendFactory::new();
            factory.open_storage(&key).map_err(ScanError::from)
        }
        other => Err(ScanError::Usage(format!("unknown backend '{other}'"))),
    }
}

/// Encodes a numeric key as fixed-width big-endian bytes (ordered).
fn key_bytes(i: u64) -> [u8; 8] {
    i.to_be_bytes()
}

/// Builds a deterministic ~200 B value.
fn value_bytes(seed: u64) -> Vec<u8> {
    let mut value = Vec::with_capacity(VALUE_LEN);
    for i in 0..VALUE_LEN {
        value.push((seed.wrapping_add(i as u64) & 0xff) as u8);
    }
    value
}

/// Ensures store `s` exists; returns its id.
fn ensure_store(storage: &dyn boa_idb_core::backend::traits::Storage) -> Result<u64, ScanError> {
    use boa_idb_core::backend::types::StoreSpec;
    use boa_idb_core::key::path::KeyPath;
    use boa_idb_core::key::utf16::Utf16String;

    let mut db = storage.open_database("scan1m").map_err(ScanError::from)?;
    if let Some(id) = db
        .metadata()
        .stores
        .iter()
        .find(|s| s.name.to_string() == "s")
        .map(|s| s.id)
    {
        db.close().map_err(ScanError::from)?;
        return Ok(id);
    }
    let mut txn = db
        .begin(TxnMode::VersionChange, &[], Durability::Default)
        .map_err(ScanError::from)?;
    txn.set_version(1).map_err(ScanError::from)?;
    let id = txn
        .create_store(&StoreSpec {
            name: Utf16String::from("s"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .map_err(ScanError::from)?;
    txn.commit().map_err(ScanError::from)?;
    db.close().map_err(ScanError::from)?;
    Ok(id)
}

/// Counts records in store `s`.
fn count_records(
    storage: &dyn boa_idb_core::backend::traits::Storage,
    store: u64,
) -> Result<u64, ScanError> {
    let mut db = storage.open_database("scan1m").map_err(ScanError::from)?;
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .map_err(ScanError::from)?;
    let count = txn
        .count(SourceRef::Store(store), &EncodedRange::all())
        .map_err(ScanError::from)?;
    txn.commit().map_err(ScanError::from)?;
    db.close().map_err(ScanError::from)?;
    Ok(count)
}

/// Builds the fixture in 10 000-record relaxed chunks; returns build seconds.
fn build_fixture(
    storage: &dyn boa_idb_core::backend::traits::Storage,
    store: u64,
    records: u64,
) -> Result<f64, ScanError> {
    let started = Instant::now();
    let mut db = storage.open_database("scan1m").map_err(ScanError::from)?;
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
        .map_err(ScanError::from)?;
    let mut since_commit = 0_u64;
    for i in 0..records {
        txn.begin_request().map_err(ScanError::from)?;
        txn.put(store, &key_bytes(i), &value_bytes(i), false)
            .map_err(ScanError::from)?;
        txn.commit_request().map_err(ScanError::from)?;
        since_commit += 1;
        if since_commit.is_multiple_of(FILL_CHUNK) {
            txn.commit().map_err(ScanError::from)?;
            txn = db
                .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
                .map_err(ScanError::from)?;
            since_commit = 0;
        }
    }
    txn.commit().map_err(ScanError::from)?;
    db.close().map_err(ScanError::from)?;
    Ok(started.elapsed().as_secs_f64())
}

/// Runs one full cursor scan; returns `(visited, elapsed)`.
///
/// Checks the deadline every [`CHECK_EVERY`] rows: on expiry the scan stops
/// and the caller reports the controlled `TIMEOUT` verdict. The wrapping
/// checksum forces every value buffer to be touched.
fn run_scan(
    storage: &dyn boa_idb_core::backend::traits::Storage,
    store: u64,
    timeout: Duration,
) -> Result<(u64, Duration, u64), ScanError> {
    let mut db = storage.open_database("scan1m").map_err(ScanError::from)?;
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .map_err(ScanError::from)?;
    let started = Instant::now();
    let (visited, checksum) = {
        let mut cursor = txn
            .scan(
                SourceRef::Store(store),
                &EncodedRange::all(),
                Direction::Next,
                false,
            )
            .map_err(ScanError::from)?;
        let mut visited = 0_u64;
        let mut checksum = 0_u64;
        let mut active = cursor.seek(CursorSeek::First).map_err(ScanError::from)?;
        while active {
            visited += 1;
            if let Some(value) = cursor.current_value() {
                checksum = checksum.wrapping_add(u64::try_from(value.len()).unwrap_or(u64::MAX));
            }
            if visited.is_multiple_of(CHECK_EVERY) && started.elapsed() > timeout {
                break;
            }
            active = cursor.step(1).map_err(ScanError::from)?;
        }
        (visited, checksum)
    };
    let elapsed = started.elapsed();
    txn.commit().map_err(ScanError::from)?;
    db.close().map_err(ScanError::from)?;
    Ok((visited, elapsed, checksum))
}

/// Runner entry point: parse, build (unless reused), scan or report.
fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let result = parse_args(&argv).and_then(|args| {
        let storage = open_storage(&args)?;
        let store = ensure_store(&*storage)?;
        println!("scan-1m: backend={} records={}", args.backend, args.records);
        let have = count_records(&*storage, store)?;
        if args.reuse && have == args.records {
            println!("scan-1m: reused fixture records={have}");
        } else {
            if args.reuse {
                println!("scan-1m: fixture has {have} records, rebuilding");
            }
            let build_s = build_fixture(&*storage, store, args.records)?;
            println!("scan-1m: fixture_build_s={build_s:.1}");
        }
        if args.build_only {
            return Ok(());
        }
        let (visited, elapsed, checksum) = run_scan(&*storage, store, args.timeout)?;
        let elapsed_s = elapsed.as_secs_f64();
        if visited < args.records {
            println!(
                "scan-1m: TIMEOUT rows_scanned={visited} elapsed_s={elapsed_s:.1} timeout_s={} checksum={checksum}",
                args.timeout.as_secs()
            );
            std::process::exit(3);
        }
        // Counts here are far below 2^53, hence exactly representable.
        #[allow(clippy::cast_precision_loss)]
        let rate = visited as f64 / elapsed_s.max(f64::EPSILON);
        println!(
            "scan-1m: RESULT records={visited} elapsed_s={elapsed_s:.1} records_per_s={rate:.0} checksum={checksum}"
        );
        Ok(())
    });
    if let Err(error) = result {
        eprintln!("scan-1m: {error}");
        std::process::exit(error.code());
    }
}
