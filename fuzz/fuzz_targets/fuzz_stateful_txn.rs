//! Stateful transaction fuzzer (M7-B): a byte script drives backend
//! transactions (savepoints, commit/abort, reads) against a `BTreeMap`
//! model. Any panic, any backend error on a well-sequenced script, and any
//! model divergence is a bug: the backend contract requires graceful
//! `BackendError`s, never panics, in any call order.
#![no_main]

use libfuzzer_sys::fuzz_target;
use boa_idb_core::backend::traits::{BackendFactory, BackendTxn};
use boa_idb_core::backend::types::StoreSpec;
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::proto::{Direction, Durability, SourceRef, StorageKey, TxnMode};
use std::collections::BTreeMap;

/// Model: committed map plus working map with a snapshot stack shadowing
/// savepoints (`begin_request` pushes, `commit_request` merges by dropping,
/// `rollback_request` restores).
#[derive(Default)]
struct Model {
    committed: BTreeMap<(u64, Vec<u8>), Vec<u8>>,
    working: BTreeMap<(u64, Vec<u8>), Option<Vec<u8>>>,
    snapshots: Vec<BTreeMap<(u64, Vec<u8>), Option<Vec<u8>>>>,
}

impl Model {
    fn effective(&self, key: &(u64, Vec<u8>)) -> Option<Vec<u8>> {
        match self.working.get(key) {
            Some(slot) => slot.clone(),
            None => self.committed.get(key).cloned(),
        }
    }

    fn put(&mut self, store: u64, key: Vec<u8>, value: Vec<u8>) {
        self.working.insert((store, key), Some(value));
    }

    fn delete(&mut self, store: u64, key: Vec<u8>) {
        self.working.insert((store, key), None);
    }

    fn clear(&mut self, store: u64) {
        for k in self
            .committed
            .keys()
            .filter(|(s, _)| *s == store)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.working.insert(k, None);
        }
        for k in self
            .working
            .keys()
            .filter(|(s, _)| *s == store)
            .cloned()
            .collect::<Vec<_>>()
        {
            self.working.insert(k, None);
        }
    }

    fn begin_request(&mut self) {
        self.snapshots.push(self.working.clone());
    }

    fn commit_request(&mut self) {
        self.snapshots.pop();
    }

    fn rollback_request(&mut self) {
        if let Some(snapshot) = self.snapshots.pop() {
            self.working = snapshot;
        }
    }

    fn commit(&mut self) {
        for (key, slot) in std::mem::take(&mut self.working) {
            match slot {
                Some(value) => {
                    self.committed.insert(key, value);
                }
                None => {
                    self.committed.remove(&key);
                }
            }
        }
        self.snapshots.clear();
    }

    fn abort(&mut self) {
        self.working.clear();
        self.snapshots.clear();
    }
}

/// Byte cursor over fuzzer input.
struct Script<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Script<'a> {
    fn next(&mut self) -> u8 {
        let byte = *self.bytes.get(self.pos).unwrap_or(&0);
        self.pos += 1;
        byte
    }

    fn next_vec(&mut self, max_len: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max_len + 1);
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.next());
        }
        out
    }
}

fuzz_target!(|data: &[u8]| {
    // Cap the script so executions stay fast under the fuzzer.
    if data.len() > 4096 {
        return;
    }
    let factory = boa_idb_memory::MemoryBackendFactory::new();
    let storage = factory
        .open_storage(&StorageKey::new("fuzz"))
        .expect("open storage");
    let mut db = storage.open_database("fuzzdb").expect("open database");
    let mut setup = db
        .begin(TxnMode::VersionChange, &[], Durability::Default)
        .expect("begin versionchange");
    let store = setup
        .create_store(&StoreSpec {
            name: Utf16String::from("s"),
            key_path: KeyPath::Empty,
            auto_increment: false,
        })
        .expect("create store");
    setup.commit().expect("commit versionchange");

    let mut model = Model::default();
    let mut script = Script { bytes: data, pos: 0 };
    let mut txn: Option<Box<dyn BackendTxn>> = None;
    let mut depth: u32 = 0;
    let mut live = false;

    // Bounded op budget keeps every execution short.
    for _ in 0..256 {
        if script.pos >= data.len() {
            break;
        }
        match script.next() % 10 {
            // Begin (only when no transaction is live).
            0 if !live => {
                let mut next = db
                    .begin(TxnMode::ReadWrite, &[store], Durability::Relaxed)
                    .expect("begin must succeed");
                next.begin_request().expect("initial savepoint");
                depth = 1;
                model.working.clear();
                model.snapshots.clear();
                model.begin_request();
                txn = Some(next);
                live = true;
            }
            // Put.
            1 | 2 if live => {
                let key = script.next_vec(4);
                let value = script.next_vec(16);
                let txn_ref = txn.as_mut().expect("live txn");
                txn_ref.begin_request().expect("savepoint");
                depth += 1;
                model.begin_request();
                txn_ref
                    .put(store, &key, &value, false)
                    .expect("put on a live txn must succeed");
                model.put(store, key, value);
                let txn_ref = txn.as_mut().expect("live txn");
                txn_ref.commit_request().expect("commit savepoint");
                depth -= 1;
                model.commit_request();
            }
            // Delete.
            3 if live => {
                let key = script.next_vec(4);
                let before = model.effective(&(store, key.clone())).is_some();
                let txn_ref = txn.as_mut().expect("live txn");
                txn_ref.begin_request().expect("savepoint");
                depth += 1;
                model.begin_request();
                let had = txn_ref
                    .delete_record(store, &key)
                    .expect("delete on a live txn must succeed");
                assert_eq!(had, before, "backend/model delete-exists divergence");
                model.delete(store, key);
                let txn_ref = txn.as_mut().expect("live txn");
                txn_ref.commit_request().expect("commit savepoint");
                depth -= 1;
                model.commit_request();
            }
            // Get: backend must agree with the model.
            4 if live => {
                let key = script.next_vec(4);
                let txn_ref = txn.as_mut().expect("live txn");
                let got = txn_ref
                    .get(store, &key)
                    .expect("get on a live txn must succeed");
                assert_eq!(
                    got,
                    model.effective(&(store, key)),
                    "backend/model read divergence"
                );
            }
            // Count over the full range.
            5 if live => {
                let txn_ref = txn.as_mut().expect("live txn");
                let counted = txn_ref
                    .count(SourceRef::Store(store), &EncodedRange::all())
                    .expect("count must succeed");
                // Effective keys: union of committed and working, with the
                // working overlay winning.
                let mut seen = std::collections::BTreeSet::new();
                for (s, k) in model.committed.keys().filter(|(s, _)| *s == store) {
                    let live = match model.working.get(&(s.clone(), k.clone())) {
                        Some(slot) => slot.is_some(),
                        None => true,
                    };
                    if live {
                        seen.insert(k.clone());
                    }
                }
                for ((_s, k), slot) in model.working.iter().filter(|((s, _), _)| *s == store) {
                    if slot.is_some() {
                        seen.insert(k.clone());
                    }
                }
                assert_eq!(
                    counted,
                    seen.len() as u64,
                    "backend/model count divergence"
                );
            }
            // Clear the store.
            9 if live => {
                let txn_ref = txn.as_mut().expect("live txn");
                txn_ref.begin_request().expect("savepoint");
                depth += 1;
                model.begin_request();
                txn_ref.clear(store).expect("clear must succeed");
                model.clear(store);
                let txn_ref = txn.as_mut().expect("live txn");
                txn_ref.commit_request().expect("commit savepoint");
                depth -= 1;
                model.commit_request();
            }
            // Rollback the innermost savepoint.
            6 if live && depth > 0 => {
                let txn_ref = txn.as_mut().expect("live txn");
                txn_ref.rollback_request().expect("rollback savepoint");
                depth -= 1;
                model.rollback_request();
            }
            // Commit the transaction.
            7 if live => {
                let txn_ref = txn.take().expect("live txn");
                // Drain savepoints first (driver-equivalent nesting).
                txn_ref.commit().expect("commit must succeed");
                live = false;
                depth = 0;
                model.commit();
            }
            // Abort the transaction.
            8 if live => {
                let txn_ref = txn.take().expect("live txn");
                txn_ref.abort().expect("abort must succeed");
                live = false;
                depth = 0;
                model.abort();
            }
            // Cursor walk to the end (exercises iteration, no model check).
            _ if live => {
                let txn_ref = txn.as_mut().expect("live txn");
                let mut cursor = txn_ref
                    .scan(
                        SourceRef::Store(store),
                        &EncodedRange::all(),
                        Direction::Next,
                        true,
                    )
                    .expect("scan must succeed");
                if cursor
                    .seek(boa_idb_core::backend::types::CursorSeek::First)
                    .expect("seek must succeed")
                {
                    let mut steps = 0u32;
                    while cursor.step(1).expect("step must succeed") {
                        steps += 1;
                        if steps > 4096 {
                            break;
                        }
                    }
                }
            }
            // No live transaction and op needs one: skip the byte.
            _ => {}
        }
    }

    if live {
        let txn_ref = txn.take().expect("live txn");
        txn_ref.commit().expect("final commit must succeed");
        model.commit();
    }

    // Final dump must equal the model exactly.
    let mut db = storage.open_database("fuzzdb").expect("reopen");
    let mut txn = db
        .begin(TxnMode::ReadOnly, &[store], Durability::Relaxed)
        .expect("begin readonly");
    let mut dumped = BTreeMap::new();
    // Scope the cursor: it borrows the transaction mutably.
    {
        let mut cursor = txn
            .scan(
                SourceRef::Store(store),
                &EncodedRange::all(),
                Direction::Next,
                false,
            )
            .expect("final scan must succeed");
        if cursor
            .seek(boa_idb_core::backend::types::CursorSeek::First)
            .expect("final seek must succeed")
        {
            loop {
                dumped.insert(
                    cursor.current_primary_key().to_vec(),
                    cursor.current_value().unwrap_or(&[]).to_vec(),
                );
                if !cursor.step(1).expect("final step must succeed") {
                    break;
                }
            }
        }
    }
    txn.commit().expect("commit readonly");
    let mut want = BTreeMap::new();
    for ((s, k), v) in &model.committed {
        if *s == store {
            want.insert(k.clone(), v.clone());
        }
    }
    assert_eq!(dumped, want, "final dump must equal the model");
});
