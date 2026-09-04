//! Crash-consistency worker for M6-B3 / R8.5.1.
//!
//! Performs a deterministic series of FS backend commits, then parks at a
//! chosen kill point so the parent can force-terminate the process and verify
//! committed-prefix recovery.
//!
//! Marker file format (UTF-8, single line):
//! `READY <kill_kind> <durable_commits>`
//!
//! After writing the marker the process parks forever (or exits cleanly when
//! `--kill-after` is beyond the last commit and `--exit-clean` is set).

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::{IndexSpec, StoreSpec};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, StorageKey, TxnMode};
use boa_idb_fs::FsBackendFactory;

fn usage() -> ! {
    eprintln!(
        "usage: boa-idb-fs-crash-worker --root DIR --marker FILE --commits N --kill-after K [--mid-txn] [--exit-clean]"
    );
    eprintln!("  K = number of durable commits before parking (0..=N)");
    eprintln!("  --mid-txn: after K durable commits, put without commit, then park");
    process::exit(2);
}

struct Args {
    root: PathBuf,
    marker: PathBuf,
    commits: u32,
    kill_after: u32,
    mid_txn: bool,
    exit_clean: bool,
}

fn parse_args() -> Args {
    let mut root = None;
    let mut marker = None;
    let mut commits = 8u32;
    let mut kill_after = 0u32;
    let mut mid_txn = false;
    let mut exit_clean = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--marker" => marker = args.next().map(PathBuf::from),
            "--commits" => {
                commits = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage());
            }
            "--kill-after" => {
                kill_after = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage());
            }
            "--mid-txn" => mid_txn = true,
            "--exit-clean" => exit_clean = true,
            _ => usage(),
        }
    }
    Args {
        root: root.unwrap_or_else(|| usage()),
        marker: marker.unwrap_or_else(|| usage()),
        commits,
        kill_after,
        mid_txn,
        exit_clean,
    }
}

fn write_marker(path: &PathBuf, kind: &str, durable: u32) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut tmp = path.clone();
    tmp.set_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).expect("create marker tmp");
        writeln!(f, "READY {kind} {durable}").expect("write marker");
        let _ = f.sync_all();
    }
    fs::rename(&tmp, path).expect("publish marker");
}

fn park_forever() -> ! {
    loop {
        thread::park();
        // If somehow unparked, keep sleeping briefly so CPU stays idle.
        thread::sleep(Duration::from_secs(3600));
    }
}

fn main() {
    let args = parse_args();
    fs::create_dir_all(&args.root).expect("create root");

    let factory = FsBackendFactory::new(&args.root);
    let key = StorageKey::new("crash-worker");
    let storage = factory.open_storage(&key).expect("open storage");
    let mut db = storage.open_database("db").expect("open db");

    let (store, index) = {
        let mut txn = db
            .begin(TxnMode::VersionChange, &[], Durability::Strict)
            .expect("vc begin");
        let store = txn
            .create_store(&StoreSpec {
                name: Utf16String::from_str("s"),
                key_path: KeyPath::Empty,
                auto_increment: true,
            })
            .expect("create store");
        let index = txn
            .create_index(
                store,
                &IndexSpec {
                    name: Utf16String::from_str("byName"),
                    key_path: KeyPath::Empty,
                    unique: true,
                    multi_entry: false,
                },
            )
            .expect("create index");
        txn.commit().expect("vc commit");
        (store, index)
    };

    // Schema alone is durable commit 0 for kill-after semantics: durable data
    // commits are numbered 1..=commits.
    if args.kill_after == 0 && !args.mid_txn {
        write_marker(&args.marker, "after_schema", 0);
        if args.exit_clean {
            return;
        }
        park_forever();
    }

    let mut durable = 0u32;
    let target_commits = if args.mid_txn {
        args.kill_after.min(args.commits)
    } else {
        args.commits
    };
    for i in 1..=target_commits {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .expect("rw begin");
        txn.begin_request().expect("begin request");
        let pk = format!("pk{i}").into_bytes();
        let val = format!("v{i}").into_bytes();
        let ik = format!("n{i}").into_bytes();
        txn.put(store, &pk, &val, false).expect("put");
        txn.index_put(index, &ik, &pk, true).expect("index put");
        txn.key_gen_set(store, f64::from(i)).expect("keygen");
        txn.commit_request().expect("commit request");
        txn.commit().expect("commit");
        durable = i;

        if durable == args.kill_after && !args.mid_txn {
            write_marker(&args.marker, "after_commit", durable);
            if args.exit_clean {
                return;
            }
            park_forever();
        }
    }

    if args.mid_txn {
        let mut txn = db
            .begin(TxnMode::ReadWrite, &[store], Durability::Strict)
            .expect("mid begin");
        txn.begin_request().expect("mid begin request");
        let next = durable.saturating_add(1);
        let pk = format!("pk{next}").into_bytes();
        txn.put(store, &pk, b"ephemeral", false).expect("mid put");
        // Do not commit — park with only `durable` visible after kill.
        write_marker(&args.marker, "mid_txn", durable);
        if args.exit_clean {
            let _ = txn.abort();
            return;
        }
        // Keep txn alive and park (drop on kill).
        std::mem::forget(txn);
        park_forever();
    }

    write_marker(&args.marker, "complete", durable);
    if args.exit_clean {
        return;
    }
    park_forever();
}
