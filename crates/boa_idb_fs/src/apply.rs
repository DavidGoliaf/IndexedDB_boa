//! Apply recovered WAL ops onto [`DbState`].

use crate::meta::decode_meta;
use crate::state::DbState;
use crate::wal::{WalFrame, WalOp};
use boa_idb_core::backend::error::BackendError;

/// Applies committed WAL frames onto an empty or partially loaded state.
pub fn apply_frames(state: &mut DbState, frames: &[WalFrame]) -> Result<(), BackendError> {
    for frame in frames {
        for op in &frame.ops {
            apply_op(state, op)?;
        }
        state.next_txn_seq = state.next_txn_seq.max(frame.txn_seq.saturating_add(1));
    }
    Ok(())
}

fn apply_op(state: &mut DbState, op: &WalOp) -> Result<(), BackendError> {
    match op {
        WalOp::Put { store, key, value } => {
            state
                .records
                .insert_mut((*store, key.clone()), value.clone());
        }
        WalOp::Delete { store, key } => {
            state.records.remove_mut(&(*store, key.clone()));
        }
        WalOp::Clear { store } => {
            let keys: Vec<_> = state
                .records
                .iter()
                .filter_map(|(k, _)| if k.0 == *store { Some(k.clone()) } else { None })
                .collect();
            for key in keys {
                state.records.remove_mut(&key);
            }
        }
        WalOp::IndexPut {
            index,
            idx_key,
            primary_key,
        } => {
            state
                .index_entries
                .insert_mut((*index, idx_key.clone(), primary_key.clone()), ());
        }
        WalOp::IndexDelete {
            index,
            idx_key,
            primary_key,
        } => {
            state
                .index_entries
                .remove_mut(&(*index, idx_key.clone(), primary_key.clone()));
        }
        WalOp::KeyGenSet { store, value_bits } => {
            let value = f64::from_bits(*value_bits);
            state.key_generators.insert(*store, value);
            if let Some(meta) = state.meta.as_mut() {
                if let Some(s) = meta.stores.iter_mut().find(|s| s.id == *store) {
                    s.key_gen = value;
                }
            }
        }
        WalOp::MetaReplace { bytes } => {
            state.meta = Some(decode_meta(bytes)?);
        }
    }
    Ok(())
}
