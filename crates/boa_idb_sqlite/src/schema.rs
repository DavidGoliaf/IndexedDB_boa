//! DDL schema definitions, pragmas, and migration support.

use boa_idb_core::backend::error::BackendError;
use rusqlite::{Connection, OptionalExtension};

/// Current schema version.
pub const SCHEMA_VERSION: u64 = 1;

/// Pragmas applied to every database on open.
pub const INIT_PRAGMAS: &str = r#"
PRAGMA page_size = 4096;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -8000;
PRAGMA wal_autocheckpoint = 1000;
"#;

/// DDL for the IndexedDB data tables.
pub const DDL_DATA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    k TEXT PRIMARY KEY,
    v BLOB
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS object_stores (
    id             INTEGER PRIMARY KEY,
    name           BLOB NOT NULL,
    key_path       BLOB,
    auto_increment INTEGER NOT NULL DEFAULT 0,
    key_gen        REAL    NOT NULL DEFAULT 1,
    UNIQUE(name)
);

CREATE TABLE IF NOT EXISTS indexes (
    id          INTEGER PRIMARY KEY,
    store_id    INTEGER NOT NULL REFERENCES object_stores(id) ON DELETE CASCADE,
    name        BLOB NOT NULL,
    key_path    BLOB NOT NULL,
    is_unique   INTEGER NOT NULL DEFAULT 0,
    multi_entry INTEGER NOT NULL DEFAULT 0,
    UNIQUE(store_id, name)
);

CREATE TABLE IF NOT EXISTS records (
    store_id INTEGER NOT NULL REFERENCES object_stores(id) ON DELETE CASCADE,
    key      BLOB NOT NULL,
    value    BLOB,
    ext      TEXT,
    vlen     INTEGER NOT NULL,
    PRIMARY KEY (store_id, key)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS index_records (
    index_id INTEGER NOT NULL REFERENCES indexes(id) ON DELETE CASCADE,
    key      BLOB NOT NULL,
    pkey     BLOB NOT NULL,
    PRIMARY KEY (index_id, key, pkey)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_index_records_pkey ON index_records(index_id, pkey);
"#;

/// DDL for the registry database.
pub const DDL_REGISTRY: &str = r#"
CREATE TABLE IF NOT EXISTS databases (
    name       BLOB PRIMARY KEY,
    version    INTEGER NOT NULL DEFAULT 0,
    db_file    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
"#;

/// Initializes a database connection with pragmas and schema.
pub fn init_database(conn: &Connection) -> Result<(), BackendError> {
    conn.execute_batch(INIT_PRAGMAS)
        .map_err(|e| BackendError::Internal(format!("Failed to set pragmas: {e}")))?;

    conn.execute_batch(DDL_DATA)
        .map_err(|e| BackendError::Internal(format!("Failed to create schema: {e}")))?;

    // Bring an existing database up to the current schema version (a no-op
    // for freshly created databases).
    migrate_if_needed(conn)?;
    Ok(())
}

/// Initializes the registry database.
pub fn init_registry(conn: &Connection) -> Result<(), BackendError> {
    conn.execute_batch(INIT_PRAGMAS)
        .map_err(|e| BackendError::Internal(format!("Failed to set pragmas: {e}")))?;

    conn.execute_batch(DDL_REGISTRY)
        .map_err(|e| BackendError::Internal(format!("Failed to create registry schema: {e}")))?;

    Ok(())
}

/// Reads the schema version from the `meta` table.
pub fn get_schema_version(conn: &Connection) -> Result<u64, BackendError> {
    let version: Option<Vec<u8>> = conn
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| BackendError::Internal(format!("Failed to read schema version: {e}")))?;

    match version {
        Some(bytes) if bytes.len() >= 8 => {
            let arr: [u8; 8] = bytes[..8]
                .try_into()
                .map_err(|_| BackendError::Corrupted("Invalid schema_version blob".into()))?;
            Ok(u64::from_le_bytes(arr))
        }
        Some(_) => Err(BackendError::Corrupted(
            "schema_version blob too short".into(),
        )),
        None => Ok(0),
    }
}

/// Writes the schema version to the `meta` table.
pub fn set_schema_version(conn: &Connection, version: u64) -> Result<(), BackendError> {
    conn.execute(
        "INSERT OR REPLACE INTO meta (k, v) VALUES ('schema_version', ?1)",
        [version.to_le_bytes().as_slice()],
    )
    .map_err(|e| BackendError::Internal(format!("Failed to set schema version: {e}")))?;
    Ok(())
}

/// Runs migrations if the schema version is older than `SCHEMA_VERSION`.
pub fn migrate_if_needed(conn: &Connection) -> Result<(), BackendError> {
    let current = get_schema_version(conn)?;
    if current < SCHEMA_VERSION {
        // Future migrations go here, e.g.:
        // if current < 2 { migrate_v1_to_v2(conn)?; }
        set_schema_version(conn, SCHEMA_VERSION)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_database() {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        let version = get_schema_version(&conn).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn test_init_registry() {
        let conn = Connection::open_in_memory().unwrap();
        init_registry(&conn).unwrap();
        // Verify table exists
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM databases", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_schema_version_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        set_schema_version(&conn, 42).unwrap();
        assert_eq!(get_schema_version(&conn).unwrap(), 42);
    }

    #[test]
    fn test_migrate_if_needed_updates_old_version() {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        set_schema_version(&conn, 0).unwrap();
        migrate_if_needed(&conn).unwrap();
        assert_eq!(get_schema_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn test_migrate_if_needed_keeps_newer_version() {
        let conn = Connection::open_in_memory().unwrap();
        init_database(&conn).unwrap();
        set_schema_version(&conn, SCHEMA_VERSION + 5).unwrap();
        migrate_if_needed(&conn).unwrap();
        assert_eq!(
            get_schema_version(&conn).unwrap(),
            SCHEMA_VERSION + 5,
            "newer versions must not be downgraded"
        );
    }
}
