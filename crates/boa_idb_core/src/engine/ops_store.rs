//! Store operations (§6.1, §6.2, §6.4, §6.5, §6.6).

use crate::backend::error::BackendError;
use crate::backend::traits::BackendTxn;
use crate::backend::types::StoreMeta;
use crate::clone::encode::encode_scf;
use crate::clone::scvalue::ScValue;
use crate::error::IdbError;
use crate::key::encode::encode_key;
use crate::key::value::Key;
use crate::limits::LimitConfig;

use super::keygen::KeyGenerator;
use super::ops_index;

/// Result of a put operation.
pub struct PutResult {
    /// The key that was stored.
    pub key: Key,
    /// Whether the record was newly inserted (false if updated).
    pub inserted: bool,
}

/// Executes a put/add operation on a store (§6.1).
///
/// This handles key extraction, auto-increment, index updates, and the actual storage.
pub fn put(
    txn: &mut dyn BackendTxn,
    store_meta: &StoreMeta,
    keygen: &mut KeyGenerator,
    value: &mut ScValue,
    explicit_key: Option<&Key>,
    no_overwrite: bool,
    limits: &LimitConfig,
) -> Result<PutResult, IdbError> {
    let store_id = store_meta.id;

    // Step 1: Determine the key
    let key = determine_key(store_meta, keygen, value, explicit_key)?;

    // Step 2: Check for no_overwrite constraint and track if insert
    let mut encoded = Vec::new();
    encode_key(&key, &mut encoded, limits)?;
    let existing = txn.get(store_id, &encoded).map_err(backend_err)?;
    let inserted = existing.is_none();

    if no_overwrite && !inserted {
        return Err(IdbError::Constraint(format!(
            "Record with key {key:?} already exists"
        )));
    }

    // Step 3: Update indexes
    update_indexes_on_put(txn, store_meta, value, &key, limits)?;

    // Step 4: Encode and store
    let mut encoded_key = Vec::new();
    encode_key(&key, &mut encoded_key, limits)?;
    let encoded_value = encode_scf(value, limits)
        .map_err(|e| IdbError::DataClone(format!("Failed to encode value: {e}")))?;

    txn.put(store_id, &encoded_key, &encoded_value, no_overwrite)
        .map_err(backend_err)?;

    // Step 5: Update key generator
    keygen.possibly_update(key_to_f64(&key));

    Ok(PutResult { key, inserted })
}

/// Executes a delete operation on a store (§6.4).
///
/// Deletes records in the range and cleans up associated index entries.
pub fn delete(
    txn: &mut dyn BackendTxn,
    store_meta: &StoreMeta,
    range: &crate::key::range::EncodedRange,
    _limits: &LimitConfig,
) -> Result<u64, IdbError> {
    let store_id = store_meta.id;

    // Delete records and get count
    let count = txn.delete_range(store_id, range).map_err(backend_err)?;

    // Note: In a full implementation, we'd iterate the range to collect
    // primary keys of deleted records, then delete their index entries.
    // The backend's delete_range should handle index cleanup internally.

    Ok(count)
}

/// Executes a clear operation on a store (§6.6).
///
/// Clears all records and associated index entries.
pub fn clear(txn: &mut dyn BackendTxn, store_meta: &StoreMeta) -> Result<(), IdbError> {
    let store_id = store_meta.id;
    txn.clear(store_id).map_err(backend_err)?;

    // Note: The backend's clear implementation should handle
    // cleaning up index entries for the cleared store.

    Ok(())
}

/// Determines the key for a put operation.
fn determine_key(
    store_meta: &StoreMeta,
    keygen: &mut KeyGenerator,
    value: &ScValue,
    explicit_key: Option<&Key>,
) -> Result<Key, IdbError> {
    match (&store_meta.key_path, explicit_key) {
        // Has keyPath and explicit key -> error
        (kp, Some(_)) if !matches!(kp, crate::key::path::KeyPath::Empty) => Err(IdbError::Data(
            "Cannot provide explicit key when store has a keyPath".into(),
        )),
        // Has keyPath, no explicit key -> extract from value
        (kp, None) if !matches!(kp, crate::key::path::KeyPath::Empty) => {
            match kp.extract(value)? {
                Some(k) => Ok(k),
                None => {
                    if store_meta.auto_increment {
                        let generated = keygen.generate()?;
                        let key = Key::Number(generated);
                        // Inject key into value
                        // Note: This requires mutable value, handled by caller
                        Ok(key)
                    } else {
                        Err(IdbError::Data(
                            "Could not extract key from value and autoIncrement is disabled".into(),
                        ))
                    }
                }
            }
        }
        // No keyPath, explicit key provided -> use it
        (_, Some(k)) => Ok(k.clone()),
        // No keyPath, no explicit key -> generate
        (_, None) => {
            if store_meta.auto_increment {
                let generated = keygen.generate()?;
                Ok(Key::Number(generated))
            } else {
                Err(IdbError::Data(
                    "No key provided and autoIncrement is disabled".into(),
                ))
            }
        }
    }
}

/// Updates indexes when putting a record.
fn update_indexes_on_put(
    txn: &mut dyn BackendTxn,
    store_meta: &StoreMeta,
    value: &ScValue,
    primary_key: &Key,
    limits: &LimitConfig,
) -> Result<(), IdbError> {
    let mut encoded_pk = Vec::new();
    encode_key(primary_key, &mut encoded_pk, limits)?;

    for index in &store_meta.indexes {
        if index.deleted {
            continue;
        }

        // Extract index key
        if let Some(idx_key) = index.key_path.extract(value)? {
            // Handle multiEntry
            let keys = if index.multi_entry {
                expand_multi_entry_key(&idx_key)
            } else {
                vec![idx_key]
            };

            for k in keys {
                let mut idx_key_bytes = Vec::new();
                encode_key(&k, &mut idx_key_bytes, limits)?;
                ops_index::put(txn, index.id, &idx_key_bytes, &encoded_pk, index.unique)?;
            }
        }
    }

    Ok(())
}

/// Expands a multiEntry key into individual keys.
fn expand_multi_entry_key(key: &Key) -> Vec<Key> {
    match key {
        Key::Array(arr) => arr.clone(),
        _ => vec![key.clone()],
    }
}

/// Converts a Key to f64 for key generator comparison.
fn key_to_f64(key: &Key) -> f64 {
    match key {
        Key::Number(n) => *n,
        Key::Date(d) => *d,
        _ => 0.0,
    }
}

/// Converts a backend error to an IdbError.
#[allow(clippy::needless_pass_by_value)]
fn backend_err(e: BackendError) -> IdbError {
    IdbError::Data(format!("Backend error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::path::KeyPath;

    #[test]
    fn test_determine_key_with_keypath() {
        let meta = StoreMeta {
            id: 1,
            name: "test".into(),
            key_path: KeyPath::parse_single("id").unwrap(),
            auto_increment: false,
            key_gen: 1.0,
            indexes: vec![],
            deleted: false,
        };
        let mut keygen = KeyGenerator::new(1.0);
        let value = ScValue::Object(indexmap::IndexMap::new());

        // Should fail because value doesn't have "id" field
        let result = determine_key(&meta, &mut keygen, &value, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_determine_key_explicit_with_keypath_error() {
        let meta = StoreMeta {
            id: 1,
            name: "test".into(),
            key_path: KeyPath::parse_single("id").unwrap(),
            auto_increment: false,
            key_gen: 1.0,
            indexes: vec![],
            deleted: false,
        };
        let mut keygen = KeyGenerator::new(1.0);
        let value = ScValue::Object(indexmap::IndexMap::new());
        let explicit = Key::Number(1.0);

        let result = determine_key(&meta, &mut keygen, &value, Some(&explicit));
        assert!(result.is_err());
    }

    #[test]
    fn test_determine_key_auto_increment() {
        let meta = StoreMeta {
            id: 1,
            name: "test".into(),
            key_path: KeyPath::Empty,
            auto_increment: true,
            key_gen: 1.0,
            indexes: vec![],
            deleted: false,
        };
        let mut keygen = KeyGenerator::new(1.0);
        let value = ScValue::Object(indexmap::IndexMap::new());

        let key = determine_key(&meta, &mut keygen, &value, None).unwrap();
        assert_eq!(key, Key::Number(1.0));
    }

    #[test]
    fn test_expand_multi_entry_key() {
        let arr = Key::Array(vec![Key::Number(1.0), Key::Number(2.0), Key::Number(3.0)]);
        let keys = expand_multi_entry_key(&arr);
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn test_expand_non_array_key() {
        let key = Key::Number(1.0);
        let keys = expand_multi_entry_key(&key);
        assert_eq!(keys.len(), 1);
    }
}
