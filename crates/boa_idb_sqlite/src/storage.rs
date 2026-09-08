//! SQLite storage implementation with registry.sqlite.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{Database, Storage};
use boa_idb_core::proto::StorageKey;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::database::SqliteDatabase;
use crate::naming;
use crate::pool::ConnectionPool;
use crate::schema;

/// SQLite storage managing multiple databases via a registry.
pub struct SqliteStorage {
    root: PathBuf,
    registry_conn: Connection,
    /// Shared connection pools keyed by registry `db_file` name.
    ///
    /// Every `open_database` must reuse the same pool so the single writer
    /// slot is visible across driver `begin` calls (each of which opens a
    /// fresh [`SqliteDatabase`] handle). Without this, concurrent RW txns
    /// each take a private writer connection and collide inside SQLite.
    pools: Mutex<HashMap<String, ConnectionPool>>,
}

impl SqliteStorage {
    /// Opens or creates a storage at the given root path.
    pub fn open(root: &Path, _key: &StorageKey) -> Result<Self, BackendError> {
        std::fs::create_dir_all(root)
            .map_err(|e| BackendError::Io(format!("Failed to create storage root: {e}")))?;

        let registry_path = root.join("registry.sqlite");
        let registry_conn = Connection::open(&registry_path)
            .map_err(|e| BackendError::Internal(format!("Failed to open registry: {e}")))?;

        registry_conn
            .execute_batch(schema::INIT_PRAGMAS)
            .map_err(|e| BackendError::Internal(format!("Failed to set registry pragmas: {e}")))?;

        schema::init_registry(&registry_conn)?;

        Ok(Self {
            root: root.to_path_buf(),
            registry_conn,
            pools: Mutex::new(HashMap::new()),
        })
    }

    /// Returns the root path.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Storage for SqliteStorage {
    fn list_databases(&self) -> Result<Vec<(String, u64)>, BackendError> {
        let mut stmt = self
            .registry_conn
            .prepare("SELECT name, version, db_file FROM databases")
            .map_err(|e| BackendError::Internal(format!("Failed to list databases: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let name_bytes: Vec<u8> = row.get(0)?;
                let version: i64 = row.get(1)?;
                let db_file: String = row.get(2)?;
                Ok((name_bytes, version as u64, db_file))
            })
            .map_err(|e| BackendError::Internal(format!("List databases query failed: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            let (name_bytes, reg_version, db_file) =
                row.map_err(|e| BackendError::Internal(format!("List row error: {e}")))?;
            let name = String::from_utf16_lossy(
                &name_bytes
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect::<Vec<_>>(),
            );
            // The registry copy goes stale at open time (upgrades bump the
            // file afterwards); serve the live version from the database
            // file, falling back to the registry copy when unreadable.
            let version = read_file_version(&self.root.join(&db_file)).unwrap_or(reg_version);
            result.push((name, version));
        }

        Ok(result)
    }

    fn open_database(&self, name: &str) -> Result<Box<dyn Database>, BackendError> {
        let name_utf16: Vec<u16> = name.encode_utf16().collect();
        let name_bytes: Vec<u8> = name_utf16.iter().flat_map(|u| u.to_le_bytes()).collect();

        // Check if database exists in registry
        let existing: Option<String> = self
            .registry_conn
            .query_row(
                "SELECT db_file FROM databases WHERE name = ?1",
                [&name_bytes],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| BackendError::Internal(format!("Registry lookup failed: {e}")))?;

        let is_new = existing.is_none();
        let db_file = match existing {
            Some(f) => f,
            None => {
                // Create new entry
                let filename = naming::db_name_to_filename(&name_utf16);
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;

                // Idempotent under races: a concurrent creator wins, we reuse.
                let inserted = self
                    .registry_conn
                    .execute(
                        "INSERT OR IGNORE INTO databases (name, version, db_file, created_at) \
                         VALUES (?1, 0, ?2, ?3)",
                        rusqlite::params![name_bytes, filename, now],
                    )
                    .map_err(|e| {
                        BackendError::Internal(format!("Failed to register database: {e}"))
                    })?;
                if inserted == 0 {
                    // Lost the race: re-read the winner's filename.
                    self.registry_conn
                        .query_row(
                            "SELECT db_file FROM databases WHERE name = ?1",
                            [&name_bytes],
                            |row| row.get(0),
                        )
                        .map_err(|e| {
                            BackendError::Internal(format!("Registry re-read failed: {e}"))
                        })?
                } else {
                    filename
                }
            }
        };

        let db_path = self.root.join(&db_file);
        let db_hash = naming::db_hash_from_filename(&db_file)
            .ok_or_else(|| BackendError::Internal("Invalid db filename".into()))?
            .to_string();

        let pool = {
            let mut pools = self.pools.lock();
            if let Some(existing) = pools.get(&db_file) {
                existing.clone()
            } else {
                let created = ConnectionPool::open(&db_path)?;
                pools.insert(db_file.clone(), created.clone());
                created
            }
        };

        // Schema migrations must run on every open, not just pool creation:
        // a cached pool would otherwise serve a file whose schema version
        // predates this build (e.g. rewound out-of-band) without migrating.
        // A busy writer (`Locked`) keeps today's behavior — no blocking, and
        // the next contention-free open retries the migration.
        //
        // Backported to the M6-B merge into main: without it
        // `test_old_schema_version_is_migrated` fails on a cached-pool
        // reopen; the full fix lives on task/m7c-ci-evidence-final
        // (`cbe6c16`).
        match pool.checkout_writer() {
            Ok(checkout) => {
                schema::migrate_if_needed(checkout.conn()?)?;
            }
            Err(BackendError::Locked) => {}
            Err(other) => return Err(other),
        }

        // For a freshly created database, store the canonical name in metadata
        // so `DatabaseMeta::name` is populated after reopen.
        if is_new {
            let checkout = pool.checkout_writer()?;
            checkout
                .conn()?
                .execute(
                    "INSERT OR REPLACE INTO meta (k, v) VALUES ('db_name', ?1)",
                    [&name_bytes],
                )
                .map_err(|e| BackendError::Internal(format!("Failed to store db name: {e}")))?;
        }

        let db = SqliteDatabase::open(pool, self.root.clone(), db_hash)?;

        // Keep the registry version in sync with the database file (upgrades
        // bump the file; `list_databases` serves the registry copy).
        //
        // The indirection through `db.metadata()` borrows nothing: `version`
        // is `Copy`.
        let file_version = db.metadata().version;
        self.registry_conn
            .execute(
                "UPDATE databases SET version = ?1 WHERE name = ?2",
                rusqlite::params![file_version as i64, name_bytes],
            )
            .map_err(|e| BackendError::Internal(format!("Failed to sync version: {e}")))?;

        Ok(Box::new(db))
    }

    fn delete_database(&self, name: &str) -> Result<(), BackendError> {
        let name_utf16: Vec<u16> = name.encode_utf16().collect();
        let name_bytes: Vec<u8> = name_utf16.iter().flat_map(|u| u.to_le_bytes()).collect();

        let db_file: Option<String> = self
            .registry_conn
            .query_row(
                "SELECT db_file FROM databases WHERE name = ?1",
                [&name_bytes],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| BackendError::Internal(format!("Registry lookup failed: {e}")))?;

        if let Some(ref file) = db_file {
            // Drop the cached pool before unlinking files so open connections
            // are released and a later reopen does not reuse a dead handle.
            self.pools.lock().remove(file);

            // Delete the database file
            let db_path = self.root.join(file);
            if db_path.exists() {
                std::fs::remove_file(&db_path)
                    .map_err(|e| BackendError::Io(format!("Failed to delete db file: {e}")))?;
            }
            // Delete WAL and SHM files
            let wal_path = self.root.join(format!("{file}-wal"));
            let shm_path = self.root.join(format!("{file}-shm"));
            let _ = std::fs::remove_file(&wal_path);
            let _ = std::fs::remove_file(&shm_path);

            // Remove blob directory
            if let Some(hash) = naming::db_hash_from_filename(file) {
                let blob_dir = self.root.join("blobs").join(hash);
                if blob_dir.exists() {
                    std::fs::remove_dir_all(&blob_dir)
                        .map_err(|e| BackendError::Io(format!("Failed to remove blob dir: {e}")))?;
                }
            }

            // Remove from registry
            self.registry_conn
                .execute("DELETE FROM databases WHERE name = ?1", [&name_bytes])
                .map_err(|e| {
                    BackendError::Internal(format!("Failed to remove from registry: {e}"))
                })?;
        }

        Ok(())
    }

    fn usage_bytes(&self) -> Result<u64, BackendError> {
        let mut total: u64 = 0;

        // Sum up database file sizes
        let mut stmt = self
            .registry_conn
            .prepare("SELECT db_file FROM databases")
            .map_err(|e| BackendError::Internal(format!("Failed to query databases: {e}")))?;

        let rows = stmt
            .query_map([], |row| {
                let db_file: String = row.get(0)?;
                Ok(db_file)
            })
            .map_err(|e| BackendError::Internal(format!("Usage query failed: {e}")))?;

        for row in rows {
            let db_file = row.map_err(|e| BackendError::Internal(format!("Row error: {e}")))?;
            let db_path = self.root.join(&db_file);
            if let Ok(meta) = std::fs::metadata(&db_path) {
                total += meta.len();
            }
            // Include WAL file
            let wal_path = self.root.join(format!("{db_file}-wal"));
            if let Ok(meta) = std::fs::metadata(&wal_path) {
                total += meta.len();
            }
        }

        // Sum up blob files
        let blobs_dir = self.root.join("blobs");
        if blobs_dir.exists() {
            total += dir_size(&blobs_dir)?;
        }

        // Registry file
        let reg_path = self.root.join("registry.sqlite");
        if let Ok(meta) = std::fs::metadata(&reg_path) {
            total += meta.len();
        }

        Ok(total)
    }
}

/// Reads the live database version from a database file's `meta` table.
///
/// Used by [`list_databases`][Storage::list_databases]: the registry copy is
/// only synced at open time, so post-open upgrades would otherwise be
/// invisible (and version-0 databases wrongly omitted).
fn read_file_version(path: &Path) -> Result<u64, BackendError> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| BackendError::Internal(format!("Failed to open db file: {e}")))?;
    let bytes: Vec<u8> = conn
        .query_row("SELECT v FROM meta WHERE k = 'version'", [], |row| {
            row.get(0)
        })
        .map_err(|e| BackendError::Internal(format!("Failed to read version: {e}")))?;
    if bytes.len() < 8 {
        return Err(BackendError::Corrupted("Invalid version blob".into()));
    }
    let arr: [u8; 8] = bytes[..8]
        .try_into()
        .map_err(|_| BackendError::Corrupted("Invalid version blob".into()))?;
    Ok(u64::from_le_bytes(arr))
}

/// Recursively calculates directory size.
fn dir_size(path: &Path) -> Result<u64, BackendError> {
    let mut total = 0u64;
    let entries = std::fs::read_dir(path)
        .map_err(|e| BackendError::Io(format!("Failed to read dir: {e}")))?;
    for entry in entries {
        let entry = entry.map_err(|e| BackendError::Io(format!("Dir entry error: {e}")))?;
        let meta = entry
            .metadata()
            .map_err(|e| BackendError::Io(format!("Metadata error: {e}")))?;
        if meta.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}
