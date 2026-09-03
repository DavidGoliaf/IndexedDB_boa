//! Connection pool: checkout model with 1 writer slot + N readers.
//!
//! Transactions own their connection for their whole lifetime (AD-6), so the
//! pool hands out owned [`Checkout`] handles instead of borrow guards: this
//! is what lets `SqliteTxn` satisfy the `'static` bound of
//! `Database::begin`. Dropping a checkout returns its connection to the pool.
//!
//! The writer slot holds at most one connection. A checkout attempt while it
//! is taken fails fast with [`BackendError::Locked`] (no blocking: the L1
//! pump is single-threaded and must never wait on a lock held by a
//! transaction it is itself driving). The L1 scheduler minimizes this by
//! never co-starting conflicting transactions; on `Locked` it retries the
//! start on a later turn.

use boa_idb_core::backend::error::BackendError;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::schema;

/// Default number of reader connections.
const DEFAULT_MAX_READERS: usize = 4;

/// Shared pool state.
struct PoolInner {
    db_path: PathBuf,
    writer: Mutex<Option<Connection>>,
    readers: Mutex<Vec<Connection>>,
    max_readers: usize,
}

/// Connection pool with 1 writer slot + N readers.
///
/// Cheaply clonable (shared `Arc` state); each `SqliteDatabase` owns one.
#[derive(Clone)]
pub struct ConnectionPool {
    inner: Arc<PoolInner>,
}

impl ConnectionPool {
    /// Opens a new connection pool for the given database file.
    pub fn open(db_path: &Path) -> Result<Self, BackendError> {
        Self::open_with_readers(db_path, DEFAULT_MAX_READERS)
    }

    /// Opens a pool with a custom reader count.
    pub fn open_with_readers(db_path: &Path, max_readers: usize) -> Result<Self, BackendError> {
        let writer_conn = Self::create_connection(db_path)?;
        schema::init_database(&writer_conn)?;

        let mut readers = Vec::with_capacity(max_readers);
        for _ in 0..max_readers {
            readers.push(Self::create_connection(db_path)?);
        }

        Ok(Self {
            inner: Arc::new(PoolInner {
                db_path: db_path.to_path_buf(),
                writer: Mutex::new(Some(writer_conn)),
                readers: Mutex::new(readers),
                max_readers,
            }),
        })
    }

    /// Creates a WAL-mode connection with standard pragmas.
    ///
    /// Each connection carries a 64-entry prepared-statement cache via
    /// rusqlite's per-connection `prepare_cached` (see ADR-005: this is the
    /// concrete mechanism behind the "LRU statement cache" requirement).
    fn create_connection(path: &Path) -> Result<Connection, BackendError> {
        let conn = Connection::open(path)
            .map_err(|e| BackendError::Internal(format!("Failed to open SQLite: {e}")))?;
        conn.set_prepared_statement_cache_capacity(
            // Default is 16; the spec requires a 64-entry LRU cache.
            64,
        );
        conn.execute_batch(schema::INIT_PRAGMAS)
            .map_err(|e| BackendError::Internal(format!("Failed to set pragmas: {e}")))?;
        Ok(conn)
    }

    /// Checks out the writer connection (for ReadWrite/VersionChange).
    ///
    /// Fails with [`BackendError::Locked`] while another transaction holds
    /// the writer slot — never blocks.
    pub fn checkout_writer(&self) -> Result<Checkout, BackendError> {
        let conn = self
            .inner
            .writer
            .lock()
            .take()
            .ok_or(BackendError::Locked)?;
        Ok(Checkout {
            conn: Some(conn),
            writer: true,
            pool: Arc::clone(&self.inner),
        })
    }

    /// Checks out a reader connection (for ReadOnly).
    ///
    /// Falls back to a fresh ad-hoc connection when the pool is exhausted.
    pub fn checkout_reader(&self) -> Result<Checkout, BackendError> {
        let conn = {
            let mut readers = self.inner.readers.lock();
            readers.pop()
        };
        let conn = match conn {
            Some(c) => c,
            None => Self::create_connection(&self.inner.db_path)?,
        };
        Ok(Checkout {
            conn: Some(conn),
            writer: false,
            pool: Arc::clone(&self.inner),
        })
    }
}

/// An owned, checked-out connection.
///
/// Dropping returns the connection to the pool (writer slot or readers).
pub struct Checkout {
    conn: Option<Connection>,
    writer: bool,
    pool: Arc<PoolInner>,
}

impl Checkout {
    /// Returns a reference to the underlying connection.
    ///
    /// `rusqlite::Connection` uses interior mutability, so shared access
    /// suffices for every backend operation.
    pub fn conn(&self) -> Result<&Connection, BackendError> {
        self.conn
            .as_ref()
            .ok_or_else(|| BackendError::Internal("checked-out connection is gone".into()))
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            if self.writer {
                *self.pool.writer.lock() = Some(conn);
            } else {
                let mut readers = self.pool.readers.lock();
                if readers.len() < self.pool.max_readers {
                    readers.push(conn);
                }
                // else: drop the connection (pool full)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn reader_count(pool: &ConnectionPool) -> usize {
        pool.inner.readers.lock().len()
    }

    #[test]
    fn test_pool_opens_and_initializes_schema() {
        let tmp = tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let pool = ConnectionPool::open(&db_path).unwrap();

        let checkout = pool.checkout_writer().unwrap();
        let count: i64 = checkout
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "schema must be initialized on the writer connection"
        );
        drop(checkout);

        // The database file exists on disk.
        assert!(db_path.exists());
    }

    #[test]
    fn test_writer_slot_exclusive() {
        let tmp = tempdir().unwrap();
        let pool = ConnectionPool::open(&tmp.path().join("t.db")).unwrap();
        let _first = pool.checkout_writer().unwrap();
        // Second checkout fails fast instead of blocking.
        let Err(err) = pool.checkout_writer() else {
            panic!("second writer checkout must fail")
        };
        assert!(matches!(err, BackendError::Locked));
    }

    #[test]
    fn test_writer_slot_returned_on_drop() {
        let tmp = tempdir().unwrap();
        let pool = ConnectionPool::open(&tmp.path().join("t.db")).unwrap();
        {
            let _first = pool.checkout_writer().unwrap();
            assert!(pool.checkout_writer().is_err());
        }
        assert!(pool.checkout_writer().is_ok());
    }

    #[test]
    fn test_zero_readers_creates_ad_hoc() {
        let tmp = tempdir().unwrap();
        let pool = ConnectionPool::open_with_readers(&tmp.path().join("t.db"), 0).unwrap();
        let checkout = pool.checkout_reader().unwrap();
        let count: i64 = checkout
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1, "ad-hoc readers see the same schema");
        drop(checkout);
        // Ad-hoc connections are never returned to a max_readers=0 pool.
        assert_eq!(reader_count(&pool), 0);
    }

    #[test]
    fn test_reader_returned_to_pool() {
        let tmp = tempdir().unwrap();
        let pool = ConnectionPool::open_with_readers(&tmp.path().join("t.db"), 2).unwrap();
        assert_eq!(reader_count(&pool), 2);
        {
            let _guard = pool.checkout_reader().unwrap();
            assert_eq!(reader_count(&pool), 1);
        }
        assert_eq!(reader_count(&pool), 2);
    }

    #[test]
    fn test_exhausted_pool_serves_ad_hoc() {
        let tmp = tempdir().unwrap();
        let pool = ConnectionPool::open_with_readers(&tmp.path().join("t.db"), 1).unwrap();
        let _r1 = pool.checkout_reader().unwrap();
        assert_eq!(reader_count(&pool), 0);
        let r2 = pool.checkout_reader().unwrap(); // served by an ad-hoc connection
        let _ = r2.conn().unwrap();
        drop(r2);
    }
}
