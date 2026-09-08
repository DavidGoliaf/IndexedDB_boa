//! Migration tests: verify schema version handling.

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, TxnMode};
use boa_idb_sqlite::SqliteBackendFactory;
use rusqlite::Connection;

#[test]
fn test_fresh_database_has_current_schema_version() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let mut db = storage.open_database("migrate_db").unwrap();

    // Version should be 0 for a fresh database
    assert_eq!(db.metadata().version, 0);

    // Set version
    let store_ids = db
        .metadata()
        .stores
        .iter()
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let mut txn = db
        .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
        .unwrap();
    txn.set_version(1).unwrap();
    txn.commit().unwrap();

    db.flush().unwrap();
    assert_eq!(db.metadata().version, 1);
}

#[test]
fn test_schema_version_persists() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());

    // Create and set version
    {
        let storage = factory
            .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
            .unwrap();
        let mut db = storage.open_database("version_db").unwrap();

        let store_ids = db
            .metadata()
            .stores
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>();
        let mut txn = db
            .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
            .unwrap();
        txn.set_version(42).unwrap();
        txn.commit().unwrap();
    }

    // Reopen and check
    {
        let factory2 = SqliteBackendFactory::new(tmp.path());
        let storage = factory2
            .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
            .unwrap();
        let db = storage.open_database("version_db").unwrap();
        assert_eq!(db.metadata().version, 42);
    }
}

#[test]
fn test_meta_table_schema_version() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    let _db = storage.open_database("meta_db").unwrap();

    // Resolve the database file via the shared helper: it sorts entries and
    // excludes WAL/SHM sidecars (`db-….sqlite-wal`), which a live pooled
    // connection keeps around. Picking an unsorted `read_dir` entry here
    // could open the WAL file as a database, yielding an empty `meta`
    // table and a spurious "schema_version should be in meta table"
    // failure (filesystem-dependent `read_dir` order).
    //
    // Backported to the M6-B merge into main: the same flake failed CI
    // there; the full fix lives on task/m7c-ci-evidence-final (`f1912a6`).
    let files = db_files(&tmp);
    assert!(!files.is_empty());

    let conn = Connection::open(&files[0]).unwrap();

    // Check that schema_version exists in meta table
    let version: Option<Vec<u8>> = conn
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |row| {
            row.get(0)
        })
        .ok();

    assert!(version.is_some(), "schema_version should be in meta table");
    let bytes = version.unwrap();
    assert!(bytes.len() >= 8);
    let arr: [u8; 8] = bytes[..8].try_into().unwrap();
    let ver = u64::from_le_bytes(arr);
    assert_eq!(ver, boa_idb_sqlite::schema::SCHEMA_VERSION);
}

#[test]
fn test_store_metadata_survives_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());

    // Create store with specific properties
    {
        let storage = factory
            .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
            .unwrap();
        let mut db = storage.open_database("meta_store_db").unwrap();

        let store_ids = db
            .metadata()
            .stores
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>();
        let mut txn = db
            .begin(TxnMode::VersionChange, &store_ids, Durability::Default)
            .unwrap();
        txn.set_version(1).unwrap();
        txn.create_store(&StoreSpec {
            name: Utf16String::from("my_store"),
            key_path: KeyPath::parse_single("id").unwrap(),
            auto_increment: true,
        })
        .unwrap();
        txn.commit().unwrap();
    }

    // Reopen and verify metadata
    {
        let factory2 = SqliteBackendFactory::new(tmp.path());
        let storage = factory2
            .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
            .unwrap();
        let db = storage.open_database("meta_store_db").unwrap();

        assert_eq!(db.metadata().stores.len(), 1);
        let store = &db.metadata().stores[0];
        assert_eq!(store.name.to_string(), "my_store");
        assert!(store.auto_increment);
    }
}

#[test]
fn test_old_schema_version_is_migrated() {
    use boa_idb_sqlite::schema;

    let tmp = tempfile::tempdir().unwrap();
    let factory = SqliteBackendFactory::new(tmp.path());
    let storage = factory
        .open_storage(&boa_idb_core::proto::StorageKey::new("test"))
        .unwrap();
    storage.open_database("migrate_me").unwrap();

    // Rewind the stored schema version to simulate an old database.
    let files: Vec<_> = db_files(&tmp);
    assert!(!files.is_empty());
    let conn = Connection::open(&files[0]).unwrap();
    schema::set_schema_version(&conn, 0).unwrap();
    assert_eq!(schema::get_schema_version(&conn).unwrap(), 0);
    drop(conn);

    // Reopening the database migrates it to the current schema version.
    let db = storage.open_database("migrate_me").unwrap();
    assert_eq!(db.metadata().version, 0, "data version stays intact");

    let conn = Connection::open(&db_files(&tmp)[0]).unwrap();
    assert_eq!(
        schema::get_schema_version(&conn).unwrap(),
        schema::SCHEMA_VERSION,
        "schema was migrated to current version"
    );
}

#[test]
fn test_registry_schema_is_usable() {
    use boa_idb_sqlite::schema;

    let tmp = tempfile::tempdir().unwrap();
    let storage_path = tmp.path().join(boa_idb_sqlite::storage_dir_name("test"));
    std::fs::create_dir_all(&storage_path).unwrap();

    let conn = Connection::open(storage_path.join("registry.sqlite")).unwrap();
    schema::init_registry(&conn).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM databases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

/// Lists the `db-*.sqlite` files inside the storage-key directory.
///
/// WAL/SHM sidecars (`db-….sqlite-wal`) are excluded: a live pooled
/// connection keeps them around and opening one as a database yields an
/// empty `meta` table.
fn db_files(tmp: &tempfile::TempDir) -> Vec<std::path::PathBuf> {
    let dir = tmp.path().join(boa_idb_sqlite::storage_dir_name("test"));
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("db-") && name.ends_with(".sqlite")
        })
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}
