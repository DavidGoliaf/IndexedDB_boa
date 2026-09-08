//! Dev-only `IndexedDB` storage inspector (M7-A observability, §12.3).
//!
//! This binary is a **development tool, not a production API**: it opens
//! backend storage directories directly (bypassing the JS engine) to list
//! databases, dump metadata/counts, verify consistency and report stats.
//! It must never grow into a supported management surface without an ADR.
//!
//! Privacy contract: `dump` prints metadata and key **counts** only. Record
//! keys and SCF value bytes are printed exclusively behind the explicit
//! `--values` flag (hex-encoded, capped per value).
//!
//! ```sh
//! boa-idb-cli --root ./idb-data --backend sqlite ls
//! boa-idb-cli --root ./idb-data --backend fs dump mydb
//! boa-idb-cli --root ./idb-data --backend fs dump mydb --values
//! boa-idb-cli --root ./idb-data --backend sqlite verify mydb
//! boa-idb-cli --root ./idb-data --backend fs compact mydb
//! boa-idb-cli --root ./idb-data --backend sqlite stats
//! ```

use std::path::PathBuf;

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendFactory, Database};
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Durability, SourceRef, StorageKey, TxnMode};

/// Max value bytes hex-dumped per record behind `--values`.
const MAX_VALUE_DUMP: usize = 256;

/// CLI failures with process exit codes (`0` success, `1` usage, `2` backend).
#[derive(Debug, thiserror::Error)]
enum CliError {
    /// Command-line usage error.
    #[error("usage: {0}")]
    Usage(String),
    /// Backend operation failed.
    #[error("backend error: {0}")]
    Backend(String),
}

impl CliError {
    /// Process exit code for this failure.
    fn code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 1,
            CliError::Backend(_) => 2,
        }
    }
}

impl From<BackendError> for CliError {
    fn from(error: BackendError) -> Self {
        CliError::Backend(error.to_string())
    }
}

/// Parsed command line.
struct Args {
    root: PathBuf,
    backend: String,
    storage_key: String,
    command: Command,
}

/// Inspector command.
enum Command {
    /// List databases with versions.
    Ls,
    /// Dump database metadata and counts (values only with `--values`).
    Dump {
        /// Database name.
        db: String,
        /// Print hex keys and capped hex values.
        values: bool,
    },
    /// Verify cursor/count consistency for every store.
    Verify {
        /// Database name.
        db: String,
    },
    /// Checkpoint (flush + close) and report usage before/after.
    Compact {
        /// Database name.
        db: String,
    },
    /// Storage usage and per-database store counts.
    Stats,
}

/// Lowercase hex encoding (local helper; no extra dependency by design).
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// Parses `std::env::args` (hand-rolled: no argument-parser dependency).
fn parse_args(argv: &[String]) -> Result<Args, CliError> {
    let usage = "boa-idb-cli --root <dir> --backend <sqlite|fs|memory> [--storage-key <key>] <ls|dump <db> [--values]|verify <db>|compact <db>|stats>";
    let mut root: Option<PathBuf> = None;
    let mut backend: Option<String> = None;
    let mut storage_key = String::from("cli");
    let mut positional: Vec<String> = Vec::new();
    let mut values = false;
    let mut iter = argv.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => {
                let dir = iter.next().ok_or_else(|| CliError::Usage(usage.into()))?;
                root = Some(PathBuf::from(dir));
            }
            "--backend" => {
                backend = Some(
                    iter.next()
                        .cloned()
                        .ok_or_else(|| CliError::Usage(usage.into()))?,
                );
            }
            "--storage-key" => {
                storage_key = iter
                    .next()
                    .cloned()
                    .ok_or_else(|| CliError::Usage(usage.into()))?;
            }
            "--values" => values = true,
            "--help" | "-h" => return Err(CliError::Usage(usage.into())),
            other => positional.push(other.to_owned()),
        }
    }
    let root = root.ok_or_else(|| CliError::Usage(usage.into()))?;
    let backend = backend.ok_or_else(|| CliError::Usage(usage.into()))?;
    if backend != "sqlite" && backend != "fs" && backend != "memory" {
        return Err(CliError::Usage(format!(
            "unknown backend '{backend}' (expected sqlite|fs|memory)"
        )));
    }
    let command = match positional.as_slice() {
        [cmd] if cmd == "ls" => Command::Ls,
        [cmd] if cmd == "stats" => Command::Stats,
        [cmd, db] if cmd == "dump" => Command::Dump {
            db: db.clone(),
            values,
        },
        [cmd, db] if cmd == "verify" => Command::Verify { db: db.clone() },
        [cmd, db] if cmd == "compact" => Command::Compact { db: db.clone() },
        _ => return Err(CliError::Usage(usage.into())),
    };
    if values && !matches!(command, Command::Dump { .. }) {
        return Err(CliError::Usage("--values is only valid with dump".into()));
    }
    Ok(Args {
        root,
        backend,
        storage_key,
        command,
    })
}

/// Opens storage for the requested backend rooted at `root`.
fn open_storage(args: &Args) -> Result<Box<dyn boa_idb_core::backend::traits::Storage>, CliError> {
    let key = StorageKey::new(args.storage_key.clone());
    match args.backend.as_str() {
        "sqlite" => {
            let factory = boa_idb_sqlite::SqliteBackendFactory::new(&args.root);
            factory.open_storage(&key).map_err(CliError::from)
        }
        "fs" => {
            let factory = boa_idb_fs::FsBackendFactory::new(&args.root);
            factory.open_storage(&key).map_err(CliError::from)
        }
        "memory" => {
            let factory = boa_idb_memory::MemoryBackendFactory::new();
            factory.open_storage(&key).map_err(CliError::from)
        }
        other => Err(CliError::Usage(format!("unknown backend '{other}'"))),
    }
}

/// Counts records visible through a store or index source.
fn count_source(db: &mut Box<dyn Database>, source: SourceRef) -> Result<u64, CliError> {
    let store = match source {
        SourceRef::Store(id) => id,
        SourceRef::Index { store, .. } => store,
    };
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .map_err(CliError::from)?;
    let count = txn
        .count(source, &EncodedRange::all())
        .map_err(CliError::from)?;
    txn.commit().map_err(CliError::from)?;
    Ok(count)
}

/// Fully walks a store with a cursor and returns the visited record count.
fn walk_store(db: &mut Box<dyn Database>, store: u64) -> Result<u64, CliError> {
    use boa_idb_core::backend::types::CursorSeek;
    use boa_idb_core::proto::Direction;
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .map_err(CliError::from)?;
    // Scope the cursor: it borrows the transaction mutably.
    let count = {
        let mut cursor = txn
            .scan(
                SourceRef::Store(store),
                &EncodedRange::all(),
                Direction::Next,
                false,
            )
            .map_err(CliError::from)?;
        let mut count = 0_u64;
        if cursor.seek(CursorSeek::First).map_err(CliError::from)? {
            count = 1;
            while cursor.step(1).map_err(CliError::from)? {
                count += 1;
            }
        }
        count
    };
    txn.commit().map_err(CliError::from)?;
    Ok(count)
}

/// Runs `ls`: database names with versions.
fn cmd_ls(args: &Args) -> Result<(), CliError> {
    let storage = open_storage(args)?;
    let mut databases = storage.list_databases().map_err(CliError::from)?;
    databases.sort();
    for (name, version) in databases {
        println!("{name}\tversion={version}");
    }
    Ok(())
}

/// Runs `dump`: metadata and counts, values only behind `--values`.
fn cmd_dump(args: &Args, db_name: &str, with_values: bool) -> Result<(), CliError> {
    let storage = open_storage(args)?;
    let mut db = storage.open_database(db_name).map_err(CliError::from)?;
    let meta = db.metadata().clone();
    println!("database: {}", meta.name);
    println!("version: {}", meta.version);
    for store in &meta.stores {
        if store.deleted {
            continue;
        }
        let count = count_source(&mut db, SourceRef::Store(store.id))?;
        println!("store: {} records={count}", store.name);
        for index in &store.indexes {
            if index.deleted {
                continue;
            }
            let index_count = count_source(
                &mut db,
                SourceRef::Index {
                    store: store.id,
                    index: index.id,
                },
            )?;
            println!("  index: {} records={index_count}", index.name);
        }
        if with_values {
            dump_values(&mut db, store.id)?;
        }
    }
    db.close().map_err(CliError::from)?;
    Ok(())
}

/// Prints hex keys and capped hex values of one store (`--values` only).
fn dump_values(db: &mut Box<dyn Database>, store: u64) -> Result<(), CliError> {
    use boa_idb_core::backend::types::CursorSeek;
    use boa_idb_core::proto::Direction;
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .map_err(CliError::from)?;
    // Scope the cursor: it borrows the transaction mutably.
    {
        let mut cursor = txn
            .scan(
                SourceRef::Store(store),
                &EncodedRange::all(),
                Direction::Next,
                false,
            )
            .map_err(CliError::from)?;
        let mut active = cursor.seek(CursorSeek::First).map_err(CliError::from)?;
        while active {
            let key_hex = hex_encode(cursor.current_key());
            match cursor.current_value() {
                Some(value) => {
                    let shown = if value.len() > MAX_VALUE_DUMP {
                        format!(
                            "{}...({} bytes total)",
                            hex_encode(&value[..MAX_VALUE_DUMP]),
                            value.len()
                        )
                    } else {
                        hex_encode(value)
                    };
                    println!("  {key_hex} = {shown}");
                }
                None => println!("  {key_hex} = <no value>"),
            }
            active = cursor.step(1).map_err(CliError::from)?;
        }
    }
    txn.commit().map_err(CliError::from)?;
    Ok(())
}

/// Runs `verify`: cursor walk must agree with `count` for every store.
fn cmd_verify(args: &Args, db_name: &str) -> Result<(), CliError> {
    let storage = open_storage(args)?;
    let mut db = storage.open_database(db_name).map_err(CliError::from)?;
    let meta = db.metadata().clone();
    let mut stores = 0_u32;
    let mut records = 0_u64;
    for store in &meta.stores {
        if store.deleted {
            continue;
        }
        stores += 1;
        let counted = count_source(&mut db, SourceRef::Store(store.id))?;
        let walked = walk_store(&mut db, store.id)?;
        if counted != walked {
            return Err(CliError::Backend(format!(
                "store '{}': count()={counted} disagrees with cursor walk={walked}",
                store.name
            )));
        }
        records += counted;
    }
    db.close().map_err(CliError::from)?;
    println!("OK {db_name}: {stores} stores, {records} records");
    Ok(())
}

/// Runs `compact`: flush + close checkpoint with usage before/after.
fn cmd_compact(args: &Args, db_name: &str) -> Result<(), CliError> {
    let storage = open_storage(args)?;
    let before = storage.usage_bytes().map_err(CliError::from)?;
    let mut db = storage.open_database(db_name).map_err(CliError::from)?;
    db.flush().map_err(CliError::from)?;
    db.close().map_err(CliError::from)?;
    let after = storage.usage_bytes().map_err(CliError::from)?;
    println!("compact {db_name}: usage {before} -> {after} bytes");
    Ok(())
}

/// Runs `stats`: storage usage and per-database store counts.
fn cmd_stats(args: &Args) -> Result<(), CliError> {
    let storage = open_storage(args)?;
    let usage = storage.usage_bytes().map_err(CliError::from)?;
    println!("usage: {usage} bytes");
    let mut databases = storage.list_databases().map_err(CliError::from)?;
    databases.sort();
    for (name, version) in databases {
        let db = storage.open_database(&name).map_err(CliError::from)?;
        let live = db.metadata().stores.iter().filter(|s| !s.deleted).count();
        println!("{name}\tversion={version}\tstores={live}");
    }
    Ok(())
}

/// CLI entry point: parse, dispatch, map failures to exit codes.
fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let result = parse_args(&argv).and_then(|args| match &args.command {
        Command::Ls => cmd_ls(&args),
        Command::Dump { db, values } => cmd_dump(&args, db, *values),
        Command::Verify { db } => cmd_verify(&args, db),
        Command::Compact { db } => cmd_compact(&args, db),
        Command::Stats => cmd_stats(&args),
    });
    if let Err(error) = result {
        eprintln!("boa-idb-cli: {error}");
        std::process::exit(error.code());
    }
}
