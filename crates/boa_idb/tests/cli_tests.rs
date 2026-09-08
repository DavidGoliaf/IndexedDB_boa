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
///
/// Cargo usually emits `examples/boa-idb-cli[.exe]`, but coverage runners
/// (e.g. `cargo llvm-cov`, which retargets and fingerprints builds) may
/// leave only hashed names (`boa-idb-cli-<hash>[.exe]`): fall back to the
/// sorted first match so the suite works in both layouts.
//
// M7-C: extracted into [`resolve_cli_exe`] so the resolution contract is
// unit-tested without executing the binary (regression: CI coverage ran
// `--tests` without building `--examples`, leaving no binary anywhere).
fn cli_exe() -> PathBuf {
    let test_exe = std::env::current_exe().expect("current test executable path must be known");
    let profile_dir = test_exe
        .parent() // deps/
        .and_then(|p| p.parent()) // debug/ or release/
        .expect("profile directory must exist");
    resolve_cli_exe(
        profile_dir,
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
    )
    .unwrap_or_else(|checked| {
        panic!(
            "boa-idb-cli example binary must exist (checked {}); \
                 run `cargo build -p boa_idb --example boa-idb-cli` first",
            checked
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" and ")
        )
    })
}

/// Pure resolution contract behind [`cli_exe`]: probe order is
/// (1) plain `examples/boa-idb-cli[.exe]` next to the test profile dir,
/// (2) sorted-first hashed `boa-idb-cli-*[.exe]` in the same dir,
/// (3) the workspace's normal `target/<profile>/examples/` (covers runners
/// that retarget or never build examples into the current target dir).
///
/// Returns `Ok(path)` on the first hit, or `Err(checked)` listing every
/// probed candidate so failures name all searched locations.
fn resolve_cli_exe(
    profile_dir: &std::path::Path,
    manifest_dir: &std::path::Path,
) -> Result<PathBuf, Vec<PathBuf>> {
    let exe_name = if cfg!(windows) {
        "boa-idb-cli.exe"
    } else {
        "boa-idb-cli"
    };
    let mut checked = Vec::new();
    let examples = profile_dir.join("examples");
    let plain = examples.join(exe_name);
    checked.push(plain.clone());
    if plain.is_file() {
        return Ok(plain);
    }
    if let Ok(entries) = std::fs::read_dir(&examples) {
        let mut hashed: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name().is_some_and(|n| {
                        n.to_string_lossy().starts_with("boa-idb-cli")
                            && p.extension().is_none_or(|e| e == "exe")
                    })
            })
            .collect();
        hashed.sort();
        if let Some(exe) = hashed.into_iter().next() {
            return Ok(exe);
        }
    }
    // Last resort: the workspace's normal target dir (plain names). Covers
    // invocations that never build examples into the current target dir
    // (e.g. `cargo llvm-cov --test cli_tests` without `--examples`).
    let profile = profile_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("debug");
    let fallback = manifest_dir
        .join("..")
        .join("..")
        .join("target")
        .join(profile)
        .join("examples")
        .join(exe_name);
    checked.push(fallback.clone());
    if fallback.is_file() {
        return Ok(fallback);
    }
    Err(checked)
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

/// Unit tests for the [`resolve_cli_exe`] probe contract (no binary runs).
///
/// Regression for the M7-C CI coverage failure: with no binary anywhere,
/// the resolver must report every probed location (not panic inside the
/// first `read_dir`), and each probe layer must win when it holds the file.
mod resolve_cli_exe_tests {
    use super::resolve_cli_exe;
    use std::path::PathBuf;

    /// Creates an empty file at `dir/name`, creating `dir` if needed.
    fn touch(dir: &std::path::Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).expect("fixture dir");
        let path = dir.join(name);
        std::fs::write(&path, []).expect("fixture file");
        path
    }

    #[test]
    fn reports_all_checked_locations_when_nothing_built() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let profile = tmp.path().join("debug");
        std::fs::create_dir_all(profile.join("examples")).expect("examples dir");
        let err = resolve_cli_exe(&profile, tmp.path()).expect_err("must fail");
        assert_eq!(err.len(), 2, "plain + workspace fallback, got {err:?}");
    }

    #[test]
    fn missing_examples_dir_still_reports_both_locations() {
        // CI coverage retargets into a dir whose `examples/` may not exist
        // at all: `read_dir` must not panic, both candidates are reported.
        let tmp = tempfile::tempdir().expect("tempdir");
        let profile = tmp.path().join("debug");
        std::fs::create_dir_all(&profile).expect("profile dir");
        let err = resolve_cli_exe(&profile, tmp.path()).expect_err("must fail");
        assert_eq!(err.len(), 2, "plain + workspace fallback, got {err:?}");
    }

    #[test]
    fn plain_binary_wins_over_hashed_and_fallback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let profile = tmp.path().join("debug");
        let exe = if cfg!(windows) {
            "boa-idb-cli.exe"
        } else {
            "boa-idb-cli"
        };
        let plain = touch(&profile.join("examples"), exe);
        touch(&profile.join("examples"), "boa-idb-cli-abc123");
        assert_eq!(resolve_cli_exe(&profile, tmp.path()), Ok(plain));
    }

    #[test]
    fn hashed_binary_is_sorted_first_match() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let profile = tmp.path().join("examples_dir_probe");
        let examples = profile.join("examples");
        touch(&examples, "boa-idb-cli-zz99");
        let first = touch(&examples, "boa-idb-cli-aa11");
        touch(&examples, "boa-idb-cli-note.txt");
        assert_eq!(resolve_cli_exe(&profile, tmp.path()), Ok(first));
    }
}
