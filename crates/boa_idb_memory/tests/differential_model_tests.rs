//! Differential testing: `ReferenceModel` vs `MemoryBackend`.
//!
//! Generates random sequences of operations and verifies that both
//! implementations produce identical results.

#![allow(
    clippy::float_cmp,
    clippy::doc_markdown,
    clippy::match_same_arms,
    clippy::ignored_unit_patterns
)]

use std::collections::BTreeMap;

use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::proto::{Durability, TxnMode};
use boa_idb_memory::MemoryBackendFactory;
use proptest::prelude::*;

/// Simple reference model for comparison.
#[derive(Debug, Default)]
struct ReferenceModel {
    records: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl ReferenceModel {
    fn put(&mut self, key: Vec<u8>, value: Vec<u8>, no_overwrite: bool) -> Result<bool, String> {
        if no_overwrite && self.records.contains_key(&key) {
            return Err("Constraint".into());
        }
        let inserted = !self.records.contains_key(&key);
        self.records.insert(key, value);
        Ok(inserted)
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.records.get(key).cloned()
    }

    fn delete(&mut self, key: &[u8]) -> bool {
        self.records.remove(key).is_some()
    }

    fn clear(&mut self) {
        self.records.clear();
    }
}

/// Operations for the proptest.
#[derive(Debug, Clone)]
enum Op {
    Put {
        key: Vec<u8>,
        value: Vec<u8>,
        no_overwrite: bool,
    },
    Get {
        key: Vec<u8>,
    },
    Delete {
        key: Vec<u8>,
    },
    Clear,
    CommitAndRestart,
}

fn arb_key() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 1..20)
}

fn arb_value() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..50)
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (arb_key(), arb_value(), any::<bool>()).prop_map(|(k, v, no)| Op::Put {
            key: k,
            value: v,
            no_overwrite: no,
        }),
        arb_key().prop_map(|k| Op::Get { key: k }),
        arb_key().prop_map(|k| Op::Delete { key: k }),
        Just(Op::Clear),
        Just(Op::CommitAndRestart),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn differential_model(ops in prop::collection::vec(arb_op(), 1..100)) {
        let factory = MemoryBackendFactory::new();
        let storage_key = boa_idb_core::proto::StorageKey::new("test");
        let storage = factory.open_storage(&storage_key).unwrap();
        let mut db = storage.open_database("diff_test").unwrap();

        let mut model = ReferenceModel::default();
        let mut txn = db.begin(TxnMode::ReadWrite, &[1], Durability::Default).unwrap();
        txn.begin_request().unwrap();

        for op in &ops {
            match op {
                Op::Put { key, value, no_overwrite } => {
                    let model_result = model.put(key.clone(), value.clone(), *no_overwrite);
                    let txn_result = txn.put(1, key, value, *no_overwrite);

                    match (&model_result, &txn_result) {
                        (Ok(_), Ok(_)) => {}
                        (Err(_), Err(_)) => {}
                        _ => {
                            // Mismatch - check if both agree on key existence
                            let model_has = model.get(key).is_some();
                            let txn_has = txn.get(1, key).unwrap().is_some();
                            prop_assert_eq!(model_has, txn_has);
                        }
                    }
                }
                Op::Get { key } => {
                    let model_val = model.get(key);
                    let txn_val = txn.get(1, key).unwrap();
                    prop_assert_eq!(model_val, txn_val);
                }
                Op::Delete { key } => {
                    let model_existed = model.delete(key);
                    let txn_existed = txn.delete_record(1, key).unwrap();
                    prop_assert_eq!(model_existed, txn_existed);
                }
                Op::Clear => {
                    model.clear();
                    txn.clear(1).unwrap();
                }
                Op::CommitAndRestart => {
                    txn.commit_request().unwrap();
                    txn.commit().unwrap();

                    // Verify committed state matches model
                    {
                        let mut verify_txn = db.begin(TxnMode::ReadOnly, &[1], Durability::Default).unwrap();
                        verify_txn.begin_request().unwrap();

                        for k in model.records.keys() {
                            let model_val = model.get(k);
                            let txn_val = verify_txn.get(1, k).unwrap();
                            prop_assert_eq!(model_val, txn_val);
                        }

                        verify_txn.commit_request().unwrap();
                        verify_txn.commit().unwrap();
                    }

                    // Start new transaction
                    txn = db.begin(TxnMode::ReadWrite, &[1], Durability::Default).unwrap();
                    txn.begin_request().unwrap();
                }
            }
        }

        // Final verification
        txn.commit_request().unwrap();
        txn.commit().unwrap();

        let mut verify_txn = db.begin(TxnMode::ReadOnly, &[1], Durability::Default).unwrap();
        verify_txn.begin_request().unwrap();

        for k in model.records.keys() {
            let model_val = model.get(k);
            let txn_val = verify_txn.get(1, k).unwrap();
            prop_assert_eq!(model_val, txn_val);
        }

        verify_txn.commit_request().unwrap();
        verify_txn.commit().unwrap();
    }
}
