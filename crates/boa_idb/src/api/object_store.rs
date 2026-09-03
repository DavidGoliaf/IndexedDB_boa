//! `IDBObjectStore` implementation.

use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue};
use boa_gc::{Finalize, Trace};

/// Native data for `IDBObjectStore`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBObjectStoreData {
    /// Store identifier.
    pub store_id: u64,
    /// Store name.
    pub name: String,
    /// Whether auto-increment is enabled.
    pub auto_increment: bool,
}

/// Puts a record into the store.
pub fn put(
    _context: &mut Context,
    _store: &JsObject,
    _value: &JsValue,
    _key: Option<&JsValue>,
) -> JsResult<JsValue> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBObjectStore.put not yet implemented")
        .into())
}

/// Adds a record to the store (no overwrite).
pub fn add(
    _context: &mut Context,
    _store: &JsObject,
    _value: &JsValue,
    _key: Option<&JsValue>,
) -> JsResult<JsValue> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBObjectStore.add not yet implemented")
        .into())
}

/// Gets a record by key.
pub fn get(_context: &mut Context, _store: &JsObject, _query: &JsValue) -> JsResult<JsValue> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBObjectStore.get not yet implemented")
        .into())
}

/// Deletes a record by key.
pub fn delete(_context: &mut Context, _store: &JsObject, _query: &JsValue) -> JsResult<JsValue> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBObjectStore.delete not yet implemented")
        .into())
}

/// Clears all records from the store.
pub fn clear(_context: &mut Context, _store: &JsObject) -> JsResult<JsValue> {
    // TODO: implement
    Err(JsNativeError::error()
        .with_message("IDBObjectStore.clear not yet implemented")
        .into())
}
