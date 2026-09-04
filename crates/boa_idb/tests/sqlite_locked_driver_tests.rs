//! Driver + `SQLite`: independent readwrite scopes must not hang on Locked.
//!
//! The scheduler allows concurrent RW on disjoint stores, but `SQLite` has a
//! single writer slot. A `BackendError::Locked` begin must requeue the txn
//! instead of leaving it stranded in `running_txns` with `backend: None`.

use boa_engine::{Context, Source};
use boa_idb::extension::IndexedDbExtension;
use boa_idb::runtime;
use boa_idb_core::proto::StorageKey;
use boa_idb_sqlite::SqliteBackendFactory;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn run_scenario(script: &str) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("sqlite-locked"))
        .backend_factory(Arc::new(SqliteBackendFactory::new(tmp.path())))
        .build()
        .expect("build extension");
    extension
        .register(&mut context)
        .expect("register extension");

    context
        .eval(Source::from_bytes(script))
        .expect("scenario eval");
    for _ in 0..8 {
        let _ = context.run_jobs();
        let _ = runtime::pump_all(&mut context);
    }
    context
        .eval(Source::from_bytes("JSON.stringify(globalThis.__out)"))
        .expect("readback")
        .to_string(&mut context)
        .expect("to_string")
        .to_std_string_escaped()
}

#[test]
fn concurrent_rw_disjoint_stores_complete_under_sqlite() {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let out = run_scenario(
            r#"
            globalThis.__out = {};
            let openReq = indexedDB.open("lib", 1);
            openReq.onupgradeneeded = () => {
                const db = openReq.result;
                db.createObjectStore("a", { keyPath: "id" });
                db.createObjectStore("b", { keyPath: "id" });
            };
            openReq.onsuccess = () => {
                const db = openReq.result;
                const txA = db.transaction("a", "readwrite");
                const txB = db.transaction("b", "readwrite");
                txA.objectStore("a").put({ id: 1, v: "A" });
                txB.objectStore("b").put({ id: 1, v: "B" });
                let done = 0;
                const mark = () => {
                    done += 1;
                    if (done === 2) {
                        globalThis.__out.ok = true;
                    }
                };
                txA.oncomplete = mark;
                txB.oncomplete = mark;
                txA.onerror = () => { globalThis.__out.errA = true; };
                txB.onerror = () => { globalThis.__out.errB = true; };
            };
            "#,
        );
        let _ = tx.send(out);
    });

    let out = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("scenario must finish within 10s (Locked strand hung the pump)");
    assert!(
        out.contains(r#""ok":true"#),
        "both disjoint RW txns must complete, out={out}"
    );
}
