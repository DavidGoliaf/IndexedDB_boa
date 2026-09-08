//! M6-B3: forced worker termination + committed-prefix recovery (R8.3.6 / R8.5.1).
//!
//! CI default: `BOA_IDB_FS_CRASH_ITERS` unset → 8 seeded iterations.
//! Nightly: `BOA_IDB_FS_CRASH_ITERS=200 cargo test -p boa_idb_fs --test m6b3_crash_tests -- --nocapture`
//!
//! On failure the panic message includes seed and a replay command.

#![allow(clippy::doc_markdown)]

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::CursorSeek;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, Durability, SourceRef, StorageKey, TxnMode};
use boa_idb_fs::FsBackendFactory;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn crash_iters() -> u32 {
    std::env::var("BOA_IDB_FS_CRASH_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8)
}

fn worker_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_boa-idb-fs-crash-worker"))
}

fn wait_marker(path: &Path, timeout: Duration) -> String {
    let start = Instant::now();
    loop {
        if path.exists()
            && let Ok(text) = fs::read_to_string(path)
            && text.starts_with("READY ")
        {
            return text;
        }
        assert!(
            start.elapsed() <= timeout,
            "timeout waiting for crash-worker marker at {} (elapsed {:?})",
            path.display(),
            start.elapsed()
        );
        thread::yield_now();
        // Bounded backoff for CI IO; not used for correctness of recovery.
        thread::sleep(Duration::from_millis(5));
    }
}

fn force_kill(child: &mut std::process::Child) {
    // Unix: Child::kill → SIGKILL. Windows: TerminateProcess.
    let _ = child.kill();
    let _ = child.wait();
}

fn open_after_kill(root: &Path) -> Result<(), BackendError> {
    let factory = FsBackendFactory::new(root);
    let key = StorageKey::new("crash-worker");
    // Brief retry if the OS is slow to release LOCK after TerminateProcess.
    let start = Instant::now();
    loop {
        match factory
            .open_storage(&key)
            .and_then(|s| s.open_database("db"))
        {
            Ok(mut db) => {
                verify_prefix(&mut *db, expected_from_env())?;
                return Ok(());
            }
            Err(BackendError::Locked) if start.elapsed() < Duration::from_secs(5) => {
                thread::yield_now();
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err),
        }
    }
}

// Stashed expected durable count for the active iteration (set before kill).
thread_local! {
    static EXPECTED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn expected_from_env() -> u32 {
    EXPECTED.with(std::cell::Cell::get)
}

fn verify_prefix(
    db: &mut dyn boa_idb_core::backend::traits::Database,
    durable: u32,
) -> Result<(), BackendError> {
    let meta = db.metadata();
    let store = meta
        .stores
        .first()
        .map(|s| s.id)
        .ok_or_else(|| BackendError::Internal("crash verify: missing store".into()))?;
    let index = meta
        .stores
        .first()
        .and_then(|s| s.indexes.first())
        .map(|i| i.id)
        .ok_or_else(|| BackendError::Internal("crash verify: missing index".into()))?;
    let mut txn = db.begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)?;

    for i in 1..=durable {
        let pk = format!("pk{i}").into_bytes();
        let val = format!("v{i}").into_bytes();
        assert_eq!(
            txn.get(store, &pk)?,
            Some(val),
            "missing durable primary pk{i}"
        );
    }

    // Next key must not be present (neither committed-beyond nor mid-txn).
    let ghost = durable.saturating_add(1);
    let gpk = format!("pk{ghost}").into_bytes();
    assert_eq!(txn.get(store, &gpk)?, None, "ghost primary after prefix");

    // Cursor over store must equal durable count.
    {
        let mut cursor = txn.scan(
            SourceRef::Store(store),
            &EncodedRange::all(),
            Direction::Next,
            false,
        )?;
        let mut n = 0u32;
        let mut active = cursor.seek(CursorSeek::First)?;
        while active {
            n += 1;
            active = cursor.step(1)?;
        }
        assert_eq!(n, durable, "store cursor count");
    }

    // Index cursor count / keys must match durable prefix.
    {
        let mut icursor = txn.scan(
            SourceRef::Index { store, index },
            &EncodedRange::all(),
            Direction::Next,
            false,
        )?;
        let mut in_count = 0u32;
        let mut active = icursor.seek(CursorSeek::First)?;
        while active {
            in_count += 1;
            let expected_ik = format!("n{in_count}").into_bytes();
            assert_eq!(icursor.current_key(), expected_ik.as_slice());
            active = icursor.step(1)?;
        }
        assert_eq!(in_count, durable, "index cursor count");
    }

    if durable > 0 {
        let kg = txn.key_gen_current(store)?;
        assert!(
            (kg - f64::from(durable)).abs() < f64::EPSILON,
            "keygen={kg} durable={durable}"
        );
    }
    Ok(())
}

fn run_one(seed: u64, commits: u32) {
    let dir = tempdir().unwrap();
    let root = dir.path().join("data");
    let marker = dir.path().join("marker.txt");
    fs::create_dir_all(&root).unwrap();

    // Map seed → kill scenario.
    let variants = commits.saturating_add(2); // 0..=commits after_commit/schema + mid_txn slots
    #[allow(clippy::cast_possible_truncation)]
    let pick = (seed % u64::from(variants.max(1))) as u32;
    let (kill_after, mid_txn) = if pick <= commits {
        (pick, false)
    } else {
        (commits / 2, true)
    };

    EXPECTED.with(|c| c.set(kill_after));

    let mut cmd = Command::new(worker_bin());
    cmd.arg("--root")
        .arg(&root)
        .arg("--marker")
        .arg(&marker)
        .arg("--commits")
        .arg(commits.to_string())
        .arg("--kill-after")
        .arg(kill_after.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if mid_txn {
        cmd.arg("--mid-txn");
    }

    let mut child = cmd.spawn().unwrap_or_else(|e| {
        panic!("spawn crash-worker failed: {e} (seed={seed})");
    });

    let marker_text = wait_marker(&marker, Duration::from_secs(30));
    force_kill(&mut child);

    let parts: Vec<_> = marker_text.split_whitespace().collect();
    assert!(
        parts.len() >= 3 && parts[0] == "READY",
        "bad marker {marker_text:?} seed={seed}"
    );
    let reported: u32 = parts[2].parse().expect("durable parse");
    assert_eq!(reported, kill_after, "marker durable mismatch seed={seed}");

    if let Err(err) = open_after_kill(&root) {
        panic!(
            "recovery failed seed={seed} kill_after={kill_after} mid_txn={mid_txn}: {err}\nreplay: BOA_IDB_FS_CRASH_ITERS=1 cargo test -p boa_idb_fs --test m6b3_crash_tests crash_matrix_seeded -- --nocapture\nmanual: {} --root {} --marker {} --commits {commits} --kill-after {kill_after}{}",
            worker_bin().display(),
            root.display(),
            marker.display(),
            if mid_txn { " --mid-txn" } else { "" }
        );
    }
}

#[test]
fn crash_matrix_seeded() {
    let iters = crash_iters();
    let commits = 6u32;
    let base_seed: u64 = std::env::var("BOA_IDB_FS_CRASH_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x00C0_FFEE);
    for i in 0..iters {
        let seed = base_seed.wrapping_add(u64::from(i));
        run_one(seed, commits);
    }
}

#[test]
fn crash_worker_clean_exit_smoke() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("data");
    let marker = dir.path().join("marker.txt");
    let status = Command::new(worker_bin())
        .arg("--root")
        .arg(&root)
        .arg("--marker")
        .arg(&marker)
        .arg("--commits")
        .arg("2")
        .arg("--kill-after")
        .arg("2")
        .arg("--exit-clean")
        .status()
        .expect("run worker clean");
    assert!(status.success());
    EXPECTED.with(|c| c.set(2));
    open_after_kill(&root).expect("open after clean worker");
}
