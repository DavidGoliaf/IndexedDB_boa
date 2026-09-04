//! Store operations (§6.1, §6.2, §6.4, §6.5, §6.6).

use crate::backend::error::BackendError;
use crate::backend::traits::BackendTxn;
use crate::backend::types::{CursorSeek, IndexMeta, StoreMeta};
use crate::clone::decode::decode_scf;
use crate::clone::encode::encode_scf;
use crate::clone::scvalue::ScValue;
use crate::error::IdbError;
use crate::key::encode::encode_key;
use crate::key::range::EncodedRange;
use crate::key::value::Key;
use crate::limits::LimitConfig;
use crate::proto::{Direction, SourceRef, StoreId};

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

    // Step 3: Sync indexes.
    //
    // The core owns index maintenance (R8.1.2): the backend only stores the
    // `(index_key, primary_key)` pairs it is told to store. When a record is
    // overwritten, index entries that are no longer produced by the new value
    // must be deleted first; otherwise stale entries would cause phantom
    // cursor hits and false `unique` violations.
    let old_value: Option<ScValue> = match &existing {
        Some(bytes) => Some(decode_scf(bytes, limits).map_err(|e| {
            IdbError::NotReadable(format!("Stored value for key {key:?} is corrupt: {e}"))
        })?),
        None => None,
    };
    sync_indexes_on_put(txn, store_meta, old_value.as_ref(), value, &key, limits)?;

    // Step 4: Encode and store
    let mut encoded_key = Vec::new();
    encode_key(&key, &mut encoded_key, limits)?;
    let encoded_value = encode_scf(value, limits)
        .map_err(|e| IdbError::DataClone(format!("Failed to encode value: {e}")))?;

    txn.put(store_id, &encoded_key, &encoded_value, no_overwrite)
        .map_err(backend_err)?;

    // Step 5: Update key generator
    keygen.possibly_update(key_to_f64(&key));

    // Step 6: Persist the generator so the next transaction continues the
    // sequence instead of re-issuing the same auto-increment keys. The write
    // goes through the backend savepoint machinery, so a request rollback
    // restores the previous generator value together with the data.
    txn.key_gen_set(store_id, keygen.current())
        .map_err(backend_err)?;

    Ok(PutResult { key, inserted })
}

/// Executes a delete operation on a store (§6.4).
///
/// Deletes records in the range and removes the index entries that belonged
/// to the deleted records. Index cleanup is computed here in the core
/// (R8.1.2): the records are scanned first so their primary keys are known.
pub fn delete(
    txn: &mut dyn BackendTxn,
    store_meta: &StoreMeta,
    range: &crate::key::range::EncodedRange,
    _limits: &LimitConfig,
) -> Result<u64, IdbError> {
    let store_id = store_meta.id;

    // Collect primary keys first; the backend only reports a count.
    let doomed = collect_range_keys(txn, store_id, range)?;

    // Delete records and get count
    let count = txn.delete_range(store_id, range).map_err(backend_err)?;

    // Remove index entries of the deleted records.
    for pkey in &doomed {
        for index in &store_meta.indexes {
            if index.deleted {
                continue;
            }
            ops_index::delete_by_primary(txn, index.id, pkey)?;
        }
    }

    Ok(count)
}

/// Executes a clear operation on a store (§6.6).
///
/// Clears all records and removes every index entry that belonged to them.
pub fn clear(txn: &mut dyn BackendTxn, store_meta: &StoreMeta) -> Result<(), IdbError> {
    let store_id = store_meta.id;

    let doomed = collect_range_keys(txn, store_id, &EncodedRange::all())?;

    txn.clear(store_id).map_err(backend_err)?;

    for pkey in &doomed {
        for index in &store_meta.indexes {
            if index.deleted {
                continue;
            }
            ops_index::delete_by_primary(txn, index.id, pkey)?;
        }
    }

    Ok(())
}

/// Collects the encoded primary keys of all records in `range`.
///
/// Used to drive core-side index cleanup for `delete`/`clear`, since the
/// backend deletion primitives only report counts.
fn collect_range_keys(
    txn: &mut dyn BackendTxn,
    store: StoreId,
    range: &EncodedRange,
) -> Result<Vec<Vec<u8>>, IdbError> {
    let mut cursor = txn
        .scan(SourceRef::Store(store), range, Direction::Next, true)
        .map_err(backend_err)?;
    let mut keys = Vec::new();
    let mut active = cursor.seek(CursorSeek::First).map_err(backend_err)?;
    while active {
        keys.push(cursor.current_key().to_vec());
        active = cursor.step(1).map_err(backend_err)?;
    }
    Ok(keys)
}

/// Determines the key for a put operation.
///
/// If the store has a keyPath and autoIncrement, and the key cannot be extracted,
/// generates a new key and injects it into the value.
fn determine_key(
    store_meta: &StoreMeta,
    keygen: &mut KeyGenerator,
    value: &mut ScValue,
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
                        // Inject key into value at the keyPath
                        if let Err(e) = kp.inject(value, &key) {
                            return Err(IdbError::Data(format!(
                                "Failed to inject key into value: {e}"
                            )));
                        }
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
        // Empty key path without explicit key -> the value itself is the key.
        (crate::key::path::KeyPath::Empty, None) => {
            match value.to_key() {
                Ok(Some(k)) => Ok(k),
                // With autoIncrement an unusable value falls back to
                // generation instead of failing.
                _ if store_meta.auto_increment => {
                    let generated = keygen.generate()?;
                    Ok(Key::Number(generated))
                }
                Ok(None) => Err(IdbError::Data("Value cannot be used as a key".into())),
                Err(e) => Err(IdbError::Data(format!("Invalid key: {e}"))),
            }
        }
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

/// Synchronizes index entries when putting a record.
///
/// Computes the index-key sets produced by the old value (if the record
/// existed) and the new value, deletes stale entries, and inserts new ones.
/// Entries produced by both are left untouched — in particular this avoids
/// false `unique` violations when a record is overwritten without changing
/// its indexed keys.
fn sync_indexes_on_put(
    txn: &mut dyn BackendTxn,
    store_meta: &StoreMeta,
    old_value: Option<&ScValue>,
    new_value: &ScValue,
    primary_key: &Key,
    limits: &LimitConfig,
) -> Result<(), IdbError> {
    let mut encoded_pk = Vec::new();
    encode_key(primary_key, &mut encoded_pk, limits)?;

    for index in &store_meta.indexes {
        if index.deleted {
            continue;
        }

        let old_keys = match old_value {
            Some(v) => index_keys_for_value(index, v, limits)?,
            None => Vec::new(),
        };
        let new_keys = index_keys_for_value(index, new_value, limits)?;

        for old in &old_keys {
            if !new_keys.contains(old) {
                ops_index::delete(txn, index.id, old, &encoded_pk)?;
            }
        }
        for new in &new_keys {
            if !old_keys.contains(new) {
                ops_index::put(txn, index.id, new, &encoded_pk, index.unique)?;
            }
        }
    }

    Ok(())
}

/// Extracts the deduplicated set of encoded index keys one index derives
/// from a value (multiEntry arrays are expanded, duplicates removed).
///
/// A value that does not produce a valid key (missing path, or an
/// un-keyable value like a plain object) is simply not indexed — this is
/// not an error (§6.1: only the store's own key validation can fail a put).
fn index_keys_for_value(
    index: &IndexMeta,
    value: &ScValue,
    limits: &LimitConfig,
) -> Result<Vec<Vec<u8>>, IdbError> {
    let mut out = Vec::new();
    let keys = if index.multi_entry {
        index.key_path.extract_multi_entry(value)?
    } else {
        let Ok(Some(idx_key)) = index.key_path.extract(value) else {
            // Missing path or un-keyable value: skip this index.
            return Ok(Vec::new());
        };
        vec![idx_key]
    };
    for k in keys {
        let mut bytes = Vec::new();
        encode_key(&k, &mut bytes, limits)?;
        if !out.contains(&bytes) {
            out.push(bytes);
        }
    }
    Ok(out)
}

/// Expands a multiEntry key into individual keys.
#[allow(dead_code)]
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
///
/// `Constraint` violations are preserved so callers (and ultimately JS via
/// `DOMException`) observe `ConstraintError` rather than a generic `DataError`.
#[allow(clippy::needless_pass_by_value)]
fn backend_err(e: BackendError) -> IdbError {
    match e {
        BackendError::Constraint(msg) => IdbError::Constraint(msg),
        other => IdbError::Data(format!("Backend error: {other}")),
    }
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
        let mut value = ScValue::Object(indexmap::IndexMap::new());

        // Should fail because value doesn't have "id" field
        let result = determine_key(&meta, &mut keygen, &mut value, None);
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
        let mut value = ScValue::Object(indexmap::IndexMap::new());
        let explicit = Key::Number(1.0);

        let result = determine_key(&meta, &mut keygen, &mut value, Some(&explicit));
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
        let mut value = ScValue::Object(indexmap::IndexMap::new());

        let key = determine_key(&meta, &mut keygen, &mut value, None).unwrap();
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
