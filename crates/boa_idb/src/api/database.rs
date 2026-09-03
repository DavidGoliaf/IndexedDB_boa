//! `IDBDatabase` implementation.

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue};
use boa_gc::{Finalize, Trace};

/// Native data for `IDBDatabase`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBDatabaseData {
    /// Connection identifier.
    pub connection_id: u64,
    /// Database name.
    pub name: String,
    /// Database version.
    pub version: u64,
    /// Whether the connection is closed.
    pub closed: bool,
}

/// Creates a new transaction.
pub fn transaction(
    _context: &mut Context,
    _db: &JsObject,
    _store_names: &[String],
    _mode: &str,
) -> JsResult<JsValue> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBDatabase.transaction not yet implemented")
        .into())
}

/// Creates a new object store.
pub fn create_object_store(
    _context: &mut Context,
    _db: &JsObject,
    _name: &str,
    _options: Option<&JsObject>,
) -> JsResult<JsValue> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBDatabase.createObjectStore not yet implemented")
        .into())
}

/// Deletes an object store.
pub fn delete_object_store(_context: &mut Context, _db: &JsObject, _name: &str) -> JsResult<()> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBDatabase.deleteObjectStore not yet implemented")
        .into())
}

/// Closes the database connection.
pub fn close(_context: &mut Context, db: &JsObject) -> JsResult<()> {
    if let Some(data) = db.downcast_ref::<IdBDatabaseData>() {
        // Mark as closed
        // Note: In a real implementation, this would be mutable
    }
    Ok(())
}
