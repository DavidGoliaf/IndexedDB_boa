//! Index operations (§6.3, multiEntry, unique).

use crate::backend::error::BackendError;
use crate::backend::traits::BackendTxn;
use crate::error::IdbError;
use crate::proto::IndexId;

/// Inserts an index entry.
///
/// If `unique` is true, checks for existing entries with the same index key.
pub fn put(
    txn: &mut dyn BackendTxn,
    index_id: IndexId,
    idx_key: &[u8],
    primary_key: &[u8],
    unique: bool,
) -> Result<(), IdbError> {
    txn.index_put(index_id, idx_key, primary_key, unique)
        .map_err(|e| match e {
            BackendError::Constraint(msg) => IdbError::Constraint(msg),
            other => IdbError::Data(format!("Index put failed: {other}")),
        })
}

/// Deletes an index entry.
pub fn delete(
    txn: &mut dyn BackendTxn,
    index_id: IndexId,
    idx_key: &[u8],
    primary_key: &[u8],
) -> Result<(), IdbError> {
    txn.index_delete(index_id, idx_key, primary_key)
        .map_err(|e| IdbError::Data(format!("Index delete failed: {e}")))
}

/// Deletes all index entries for a primary key.
pub fn delete_by_primary(
    txn: &mut dyn BackendTxn,
    index_id: IndexId,
    primary_key: &[u8],
) -> Result<(), IdbError> {
    txn.index_delete_by_primary(index_id, primary_key)
        .map_err(|e| IdbError::Data(format!("Index delete by primary failed: {e}")))
}
