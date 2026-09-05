//! M7-A dev-CLI tests: `ls`, `dump` (metadata-only default vs `--values`),
//! `verify`, `compact` and `stats` against seeded `SQLite` and FS storage.
//!
//! The CLI is an example target, so tests locate it through the current test
//! executable's directory rather than `CARGO_BIN_EXE_*` (bins only).

#![cfg(feature = "memory")]

use std::path::PathBuf;
use std::process::Command;

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, StorageKey, TxnMode};

/// Distinctive payload: must stay out of default `dump` output.
const SECRET_VALUE: &[u8] = b"TOP-SECRET-VALUE-12345";
/// Hex prefix of [`SECRET_VALUE`] (`TOP-SECR`).
const SECRET_HEX_PREFIX: &str = "544f502d53454352";
/// Hex of the first seeded key (`cli-key-1`).
const KEY_HEX: &str = "636c692d6b65792d31";

/// Locates the built `boa-idb-cli` example executable.
fn cli_exe() -> PathBuf {
    let test_exe = std::env::current_exe().expect("current test executable path must be known");
    let profile_dir = test_exe
        .parent() // deps/
        .and_then(|p| p.parent()) // debug/ or release/
        .expect("profile directory must exist");
    let exe_name = if cfg!(windows) {
        "boa-idb-cli.exe"
    } else {
        "boa-idb-cli"
    };
    profile_dir.join("examples").join(exe_name)
}

/// Runs the CLI with `args`, asserting success and returning stdout.
fn run_cli(args: &[&str]) -> String {
    let output = Command::new(cli_exe())
        .args(args)
        .output()
        .expect("CLI example must run");
    assert!(
        output.status.success(),
        "CLI {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("CLI output must be UTF-8")
}

/// Runs the CLI expecting failure; returns the exit code.
fn run_cli_fails(args: &[&str]) -> i32 {
    let output = Command::new(cli_exe())
        .args(args)
        .output()
        .expect("CLI example must run");
    assert!(
        !output.status.success(),
        "CLI {args:?} unexpectedly succeeded"
    );
    output.status.code().unwrap_or(-1)
}

/// Seeds two raw records into store `items` of database `shop`.
///
/// Uses the CLI's default storage key (`cli`) so the seeded data is visible
/// without extra flags.
fn seed_db(factory: &dyn BackendFactory) {
    let storage = factory
        .open_storage(&StorageKey::new("cli"))
        .expect("open storage");
    let mut db = storage.open_database("shop").expect("open database");
    let mut txn = db
        .begin(TxnMode::VersionChange, &[], Durability::Default)
        .expect("begin versionchange");
    txn.set_version(1).expect("set version");
    let store = txn
        .create_store(&StoreSpec {
            name: Utf16String::from("items"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .expect("create store");
    txn.commit().expect("commit versionchange");
    let mut txn = db
        .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
        .expect("begin readwrite");
    txn.begin_request().expect("begin request");
    txn.put(store, b"cli-key-1", SECRET_VALUE, false)
        .expect("put 1");
    txn.commit_request().expect("commit request");
    txn.begin_request().expect("begin request");
    txn.put(store, b"cli-key-2", b"public-value", false)
        .expect("put 2");
    txn.commit_request().expect("commit request");
    txn.commit().expect("commit");
    db.close().expect("close");
}

#[test]
fn cli_sqlite_full_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_string_lossy().into_owned();
    seed_db(&boa_idb_sqlite::SqliteBackendFactory::new(&root));

    let ls = run_cli(&["--root", &root, "--backend", "sqlite", "ls"]);
    assert!(ls.contains("shop"), "ls lists shop: {ls}");

    let dump = run_cli(&["--root", &root, "--backend", "sqlite", "dump", "shop"]);
    assert!(dump.contains("store: items"), "dump names store: {dump}");
    assert!(dump.contains("records=2"), "dump counts records: {dump}");
    assert!(
        !dump.contains(SECRET_HEX_PREFIX),
        "default dump hides values: {dump}"
    );
    assert!(!dump.contains(KEY_HEX), "default dump hides keys: {dump}");

    let dump_values = run_cli(&[
        "--root",
        &root,
        "--backend",
        "sqlite",
        "dump",
        "shop",
        "--values",
    ]);
    assert!(
        dump_values.contains(SECRET_HEX_PREFIX),
        "--values reveals payloads: {dump_values}"
    );
    assert!(
        dump_values.contains(KEY_HEX),
        "--values reveals keys: {dump_values}"
    );

    let verify = run_cli(&["--root", &root, "--backend", "sqlite", "verify", "shop"]);
    assert!(verify.contains("OK shop"), "verify passes: {verify}");

    let compact = run_cli(&["--root", &root, "--backend", "sqlite", "compact", "shop"]);
    assert!(
        compact.contains("usage"),
        "compact reports usage: {compact}"
    );

    let stats = run_cli(&["--root", &root, "--backend", "sqlite", "stats"]);
    assert!(stats.contains("usage:"), "stats reports usage: {stats}");
    assert!(stats.contains("shop"), "stats lists shop: {stats}");
}

#[test]
fn cli_fs_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_string_lossy().into_owned();
    seed_db(&boa_idb_fs::FsBackendFactory::new(&root));

    let ls = run_cli(&["--root", &root, "--backend", "fs", "ls"]);
    assert!(ls.contains("shop"), "ls lists shop: {ls}");

    let dump = run_cli(&["--root", &root, "--backend", "fs", "dump", "shop"]);
    assert!(dump.contains("records=2"), "dump counts records: {dump}");
    assert!(
        !dump.contains(SECRET_HEX_PREFIX),
        "default dump hides values: {dump}"
    );

    let verify = run_cli(&["--root", &root, "--backend", "fs", "verify", "shop"]);
    assert!(verify.contains("OK shop"), "verify passes: {verify}");

    let dump_values = run_cli(&[
        "--root",
        &root,
        "--backend",
        "fs",
        "dump",
        "shop",
        "--values",
    ]);
    assert!(
        dump_values.contains(KEY_HEX),
        "--values reveals keys: {dump_values}"
    );
}

#[test]
fn cli_rejects_bad_usage() {
    // Unknown backend.
    assert_ne!(
        run_cli_fails(&["--root", ".", "--backend", "rocksdb", "ls"]),
        0
    );
    // Missing command.
    assert_ne!(run_cli_fails(&["--root", ".", "--backend", "sqlite"]), 0);
    // --values outside dump.
    assert_ne!(
        run_cli_fails(&["--root", ".", "--backend", "sqlite", "ls", "--values"]),
        0
    );
    // A file (not a directory) as root surfaces a backend failure (exit 2).
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let root = tmp.path().to_string_lossy().into_owned();
    assert_eq!(
        run_cli_fails(&["--root", &root, "--backend", "sqlite", "ls"]),
        2
    );
}
