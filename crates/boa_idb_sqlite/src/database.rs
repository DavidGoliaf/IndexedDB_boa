//! SQLite database implementation.

use boa_idb_core::backend::capabilities::BackendCapabilities;
use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::{BackendTxn, Database};
use boa_idb_core::backend::types::{DatabaseMeta, IndexMeta, StoreMeta};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Durability, IndexId, StoreId, TxnMode};
use rusqlite::{Connection, OptionalExtension};

use crate::blob::BlobManager;
use crate::pool::ConnectionPool;
use crate::txn::SqliteTxn;

/// SQLite database implementation.
pub struct SqliteDatabase {
    meta: DatabaseMeta,
    pool: ConnectionPool,
    storage_root: std::path::PathBuf,
    db_hash: String,
}

impl SqliteDatabase {
    /// Opens or creates a database from the given pool.
    pub fn open(
        pool: ConnectionPool,
        storage_root: std::path::PathBuf,
        db_hash: String,
    ) -> Result<Self, BackendError> {
        let meta = {
            let checkout = pool.checkout_reader()?;
            Self::load_metadata(checkout.conn()?)?
        };

        Ok(Self {
            meta,
            pool,
            storage_root,
            db_hash,
        })
    }

    /// Loads database metadata from SQLite.
    fn load_metadata(conn: &Connection) -> Result<DatabaseMeta, BackendError> {
        // Read version from meta table
        let version: u64 = conn
            .query_row("SELECT v FROM meta WHERE k = 'version'", [], |row| {
                let bytes: Vec<u8> = row.get(0)?;
                Ok(bytes)
            })
            .optional()
            .map_err(|e| BackendError::Internal(format!("Failed to read version: {e}")))?
            .map(|bytes| {
                if bytes.len() >= 8 {
                    let arr: [u8; 8] = bytes[..8]
                        .try_into()
                        .map_err(|_| BackendError::Corrupted("Invalid version blob".into()))?;
                    Ok(u64::from_le_bytes(arr))
                } else {
                    Ok(0u64)
                }
            })
            .transpose()?
            .unwrap_or(0);

        // Load object stores
        let mut stores = Vec::new();
        let mut next_store_id: StoreId = 1;
        let mut next_index_id: IndexId = 1;

        {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, key_path, auto_increment, key_gen FROM object_stores ORDER BY id",
                )
                .map_err(|e| {
                    BackendError::Internal(format!("Failed to prepare store query: {e}"))
                })?;

            let rows = stmt
                .query_map([], |row| {
                    let id: StoreId = row.get(0)?;
                    let name_bytes: Vec<u8> = row.get(1)?;
                    let key_path_bytes: Option<Vec<u8>> = row.get(2)?;
                    let auto_inc: i32 = row.get(3)?;
                    let key_gen: f64 = row.get(4)?;
                    Ok((id, name_bytes, key_path_bytes, auto_inc, key_gen))
                })
                .map_err(|e| BackendError::Internal(format!("Store query failed: {e}")))?;

            for row in rows {
                let (id, name_bytes, key_path_bytes, auto_inc, key_gen) =
                    row.map_err(|e| BackendError::Internal(format!("Store row error: {e}")))?;

                let name = bytes_to_utf16(&name_bytes);
                let key_path = key_path_bytes
                    .map(|b| bytes_to_key_path(&b))
                    .transpose()?
                    .unwrap_or(KeyPath::Empty);

                // Load indexes for this store
                let indexes = Self::load_indexes(conn, id)?;
                for idx in &indexes {
                    next_index_id = next_index_id.max(idx.id + 1);
                }

                stores.push(StoreMeta {
                    id,
                    name,
                    key_path,
                    auto_increment: auto_inc != 0,
                    key_gen,
                    indexes,
                    deleted: false,
                });
                next_store_id = next_store_id.max(id + 1);
            }
        }

        // Read name from meta
        let name: Utf16String = conn
            .query_row("SELECT v FROM meta WHERE k = 'db_name'", [], |row| {
                let bytes: Vec<u8> = row.get(0)?;
                Ok(bytes)
            })
            .optional()
            .map_err(|e| BackendError::Internal(format!("Failed to read db_name: {e}")))?
            .map(|bytes| bytes_to_utf16(&bytes))
            .unwrap_or_default();

        Ok(DatabaseMeta {
            name,
            version,
            stores,
            next_store_id,
            next_index_id,
        })
    }

    /// Loads indexes for a given store.
    fn load_indexes(conn: &Connection, store_id: StoreId) -> Result<Vec<IndexMeta>, BackendError> {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, key_path, is_unique, multi_entry \
                 FROM indexes WHERE store_id = ?1 ORDER BY id",
            )
            .map_err(|e| BackendError::Internal(format!("Failed to prepare index query: {e}")))?;

        let rows = stmt
            .query_map([store_id], |row| {
                let id: IndexId = row.get(0)?;
                let name_bytes: Vec<u8> = row.get(1)?;
                let key_path_bytes: Vec<u8> = row.get(2)?;
                let is_unique: i32 = row.get(3)?;
                let multi_entry: i32 = row.get(4)?;
                Ok((id, name_bytes, key_path_bytes, is_unique, multi_entry))
            })
            .map_err(|e| BackendError::Internal(format!("Index query failed: {e}")))?;

        let mut indexes = Vec::new();
        for row in rows {
            let (id, name_bytes, key_path_bytes, is_unique, multi_entry) =
                row.map_err(|e| BackendError::Internal(format!("Index row error: {e}")))?;

            let name = bytes_to_utf16(&name_bytes);
            let key_path = bytes_to_key_path(&key_path_bytes)?;

            indexes.push(IndexMeta {
                id,
                store_id,
                name,
                key_path,
                unique: is_unique != 0,
                multi_entry: multi_entry != 0,
                deleted: false,
            });
        }

        Ok(indexes)
    }
}

impl Database for SqliteDatabase {
    /// Snapshot of the database metadata as of the last `open`, `begin` or
    /// `flush` on this handle.
    fn metadata(&self) -> &DatabaseMeta {
        &self.meta
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            snapshot_isolation: true,
            durable: true,
            concurrent: true,
            max_key_size: None,
            max_value_size: None,
        }
    }

    fn begin(
        &mut self,
        mode: TxnMode,
        scope: &[StoreId],
        _durability: Durability,
    ) -> Result<Box<dyn BackendTxn + 'static>, BackendError> {
        // Refresh the cached metadata from the last committed state so every
        // transaction starts from a consistent snapshot of the schema.
        {
            let checkout = self.pool.checkout_reader()?;
            self.meta = Self::load_metadata(checkout.conn()?)?;
        }

        let scope_vec = scope.to_vec();
        let meta = self.meta.clone();
        let blob_manager = BlobManager::new(&self.storage_root, &self.db_hash);

        match mode {
            TxnMode::ReadOnly => {
                let checkout = self.pool.checkout_reader()?;
                let txn = SqliteTxn::new_reader(checkout, scope_vec, meta, Some(blob_manager))?;
                Ok(Box::new(txn))
            }
            TxnMode::ReadWrite | TxnMode::VersionChange => {
                let checkout = self.pool.checkout_writer()?;
                let txn =
                    SqliteTxn::new_writer(checkout, mode, scope_vec, meta, Some(blob_manager))?;
                Ok(Box::new(txn))
            }
        }
    }

    fn flush(&mut self) -> Result<(), BackendError> {
        // WAL checkpoint
        let checkout = self.pool.checkout_writer()?;
        checkout
            .conn()?
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|e| BackendError::Internal(format!("WAL checkpoint failed: {e}")))?;
        // Refresh the metadata view: like `open`/`begin`, `flush` is a
        // synchronization point. (`metadata()` is a snapshot as of the last
        // open/begin/flush on this handle — the in-memory backend shares
        // these exact semantics.)
        let checkout = self.pool.checkout_reader()?;
        self.meta = Self::load_metadata(checkout.conn()?)?;
        Ok(())
    }

    fn close(self: Box<Self>) -> Result<(), BackendError> {
        // Connections are dropped automatically
        Ok(())
    }
}

/// Converts UTF-16LE bytes to a `Utf16String`.
fn bytes_to_utf16(bytes: &[u8]) -> Utf16String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    Utf16String::from(units)
}

/// Converts stored bytes back to a `KeyPath`.
///
/// Mirrors the tagged encoding produced by `crate::txn::encode_key_path`:
/// `0x01` single path, `0x02` array path; empty bytes mean `KeyPath::Empty`.
fn bytes_to_key_path(bytes: &[u8]) -> Result<KeyPath, BackendError> {
    if bytes.is_empty() {
        return Ok(KeyPath::Empty);
    }

    match bytes[0] {
        0x01 => {
            let units = decode_utf16le(&bytes[1..])?;
            let s = String::from_utf16_lossy(&units);
            KeyPath::parse_single(&s)
                .map_err(|e| BackendError::Corrupted(format!("Invalid key path: {e}")))
        }
        0x02 => {
            let mut rest = &bytes[1..];
            let mut paths = Vec::new();
            while !rest.is_empty() {
                if rest.len() < 4 {
                    return Err(BackendError::Corrupted(
                        "Invalid array key path (truncated length)".into(),
                    ));
                }
                let len_bytes: [u8; 4] = match rest[..4].try_into() {
                    Ok(b) => b,
                    Err(_) => {
                        return Err(BackendError::Corrupted(
                            "Invalid array key path (length)".into(),
                        ));
                    }
                };
                let byte_len = u32::from_le_bytes(len_bytes) as usize;
                rest = &rest[4..];
                if rest.len() < byte_len {
                    return Err(BackendError::Corrupted(
                        "Invalid array key path (truncated data)".into(),
                    ));
                }
                let units = decode_utf16le(&rest[..byte_len])?;
                let path = Utf16String::from(units);
                KeyPath::parse_single(&path.to_string())
                    .map_err(|e| BackendError::Corrupted(format!("Invalid key path: {e}")))?;
                paths.push(path);
                rest = &rest[byte_len..];
            }
            Ok(KeyPath::Array(paths))
        }
        tag => Err(BackendError::Corrupted(format!(
            "Unknown key path tag: {tag}"
        ))),
    }
}

/// Decodes UTF-16LE bytes into a vector of code units.
fn decode_utf16le(bytes: &[u8]) -> Result<Vec<u16>, BackendError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(BackendError::Corrupted("Odd-length UTF-16 data".into()));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}
