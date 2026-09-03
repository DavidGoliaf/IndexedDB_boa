//! End-to-end `IndexedDB` flows on the memory backend.
//!
//! Each test runs a JS scenario (handlers assigned synchronously, as authors
//! do), drains the Boa job queue (executing the IDB pump), and asserts the
//! outcomes captured in globals.

use boa_engine::{Context, Source};
use boa_idb::extension::IndexedDbExtension;
use boa_idb_core::proto::StorageKey;
use boa_idb_memory::MemoryBackendFactory;
use std::sync::Arc;

fn create_context() -> Context {
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("flow-tests"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .build()
        .expect("Failed to build extension");
    extension
        .register(&mut context)
        .expect("Failed to register extension");
    context
}

/// Runs a scenario script, drains jobs, and returns a JSON-ish summary read
/// back through `globalThis.__out`.
fn run_scenario(context: &mut Context, script: &str) -> String {
    context
        .eval(Source::from_bytes(script))
        .expect("scenario eval should succeed");
    context.run_jobs().expect("jobs should run");
    // One more drain: handlers may have enqueued follow-up transactions.
    context.run_jobs().expect("jobs should run");
    context
        .eval(Source::from_bytes("JSON.stringify(globalThis.__out)"))
        .expect("readback should succeed")
        .to_string(context)
        .expect("to_string")
        .to_std_string_escaped()
}

#[test]
fn upgrade_creates_stores_and_put_get_roundtrip() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = {};
        let openReq = indexedDB.open("lib", 1);
        openReq.onupgradeneeded = (e) => {
            const db = openReq.result;
            db.createObjectStore("books", { keyPath: "isbn" });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction("books", "readwrite");
            const putReq = tx.objectStore("books").put({ isbn: 1, title: "A" });
            putReq.onsuccess = () => {
                const tx2 = db.transaction("books", "readonly");
                const getReq = tx2.objectStore("books").get(1);
                getReq.onsuccess = () => {
                    globalThis.__out.title = getReq.result && getReq.result.title;
                    globalThis.__out.key = getReq.result && getReq.result.isbn;
                };
            };
            tx.oncomplete = () => { globalThis.__out.txComplete = true; };
        };
        "#,
    );
    assert!(out.contains(r#""title":"A""#), "out={out}");
    assert!(out.contains(r#""key":1"#), "out={out}");
    assert!(out.contains(r#""txComplete":true"#), "out={out}");
}

#[test]
fn unique_violation_with_prevent_default_keeps_txn_alive() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = {};
        let openReq = indexedDB.open("lib2", 1);
        openReq.onupgradeneeded = () => {
            const s = openReq.result.createObjectStore("books", { keyPath: "id" });
            s.createIndex("by_title", "title", { unique: true });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction("books", "readwrite");
            const books = tx.objectStore("books");
            books.put({ id: 1, title: "Same" });
            const bad = books.add({ id: 2, title: "Same" });
            bad.onerror = (e) => {
                globalThis.__out.errName = bad.error && bad.error.name;
                e.preventDefault();
            };
            tx.oncomplete = () => { globalThis.__out.txComplete = true; };
            tx.onabort = () => { globalThis.__out.txAborted = true; };
        };
        "#,
    );
    assert!(out.contains(r#""errName":"ConstraintError""#), "out={out}");
    assert!(out.contains(r#""txComplete":true"#), "out={out}");
    assert!(!out.contains("txAborted"), "out={out}");
}

#[test]
fn multientry_index_and_cursor_iteration() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = { keys: [] };
        let openReq = indexedDB.open("lib3", 1);
        openReq.onupgradeneeded = () => {
            const m = openReq.result.createObjectStore("mags", { autoIncrement: true });
            m.createIndex("by_tag", "tags", { multiEntry: true });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction("mags", "readwrite");
            tx.objectStore("mags").put({ tags: ["rust", "db", "rust"] });
            tx.oncomplete = () => {
                const tx2 = db.transaction("mags", "readonly");
                const idx = tx2.objectStore("mags").index("by_tag");
                const cursorReq = idx.openCursor();
                cursorReq.onsuccess = (e) => {
                    const c = cursorReq.result;
                    if (c) {
                        globalThis.__out.keys.push(String(c.primaryKey));
                        c.continue();
                    } else {
                        globalThis.__out.done = true;
                    }
                };
            };
        };
        "#,
    );
    // "rust" collapses (dedup) → two distinct index keys, both pointing at
    // primary key 1.
    assert!(out.contains(r#""done":true"#), "out={out}");
    assert!(out.contains(r#"["1","1"]"#), "out={out}");
}

#[test]
fn same_handles_and_request_state() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = {};
        let openReq = indexedDB.open("lib4", 1);
        openReq.onupgradeneeded = () => {
            openReq.result.createObjectStore("s", { keyPath: "id" });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction("s", "readwrite");
            const a = tx.objectStore("s");
            const b = tx.objectStore("s");
            globalThis.__out.sameStore = (a === b);
            const req = a.put({ id: 7 });
            globalThis.__out.pendingState = req.readyState;
            req.onsuccess = () => {
                globalThis.__out.doneState = req.readyState;
                globalThis.__out.putKey = req.result;
            };
        };
        "#,
    );
    assert!(out.contains(r#""sameStore":true"#), "out={out}");
    assert!(out.contains(r#""pendingState":"pending""#), "out={out}");
    assert!(out.contains(r#""doneState":"done""#), "out={out}");
    assert!(out.contains(r#""putKey":7"#), "out={out}");
}

#[test]
fn pending_result_throws_invalid_state() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = {};
        let openReq = indexedDB.open("lib5", 1);
        openReq.onupgradeneeded = () => {
            openReq.result.createObjectStore("s", { keyPath: "id" });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const req = db.transaction("s", "readonly").objectStore("s").get(1);
            try {
                req.result;
                globalThis.__out.threw = false;
            } catch (e) {
                globalThis.__out.threw = (e && e.name) || String(e);
            }
        };
        "#,
    );
    assert!(
        out.contains("InvalidStateError"),
        "reading pending result must throw InvalidStateError, out={out}"
    );
}

#[test]
fn add_event_listener_path_and_keyrange() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = { seen: [] };
        let openReq = indexedDB.open("lib6", 1);
        openReq.onupgradeneeded = () => {
            openReq.result.createObjectStore("s", { keyPath: "id" });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction("s", "readwrite");
            const store = tx.objectStore("s");
            store.put({ id: 1 });
            store.put({ id: 2 });
            store.put({ id: 3 });
            tx.oncomplete = () => {
                const tx2 = db.transaction("s", "readonly");
                const range = IDBKeyRange.bound(1, 3);
                const req = tx2.objectStore("s").getAllKeys(range);
                req.addEventListener("success", () => {
                    const keys = req.result;
                    globalThis.__out.seen = [keys.length, keys[0], keys[2]];
                });
            };
        };
        "#,
    );
    assert!(out.contains(r#""seen":[3,1,3]"#), "out={out}");
}

#[test]
fn inactive_transaction_rejects_new_requests() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = {};
        let openReq = indexedDB.open("lib7", 1);
        openReq.onupgradeneeded = () => {
            openReq.result.createObjectStore("s", { keyPath: "id" });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction("s", "readonly");
            tx.abort();
            try {
                tx.objectStore("s").get(1);
                globalThis.__out.threw = false;
            } catch (e) {
                globalThis.__out.threw = (e && e.name) || String(e);
            }
        };
        "#,
    );
    // objectStore() on a finished txn throws InvalidStateError.
    assert!(out.contains("InvalidStateError"), "out={out}");
}

#[test]
fn delete_and_clear_flow() {
    let mut context = create_context();
    let out = run_scenario(
        &mut context,
        r#"
        globalThis.__out = {};
        let openReq = indexedDB.open("lib8", 1);
        openReq.onupgradeneeded = () => {
            openReq.result.createObjectStore("s", { keyPath: "id" });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction("s", "readwrite");
            const store = tx.objectStore("s");
            store.put({ id: 1 });
            store.put({ id: 2 });
            tx.oncomplete = () => {
                const tx2 = db.transaction("s", "readwrite");
                const delReq = tx2.objectStore("s").delete(1);
                delReq.onsuccess = () => {
                    const tx3 = db.transaction("s", "readonly");
                    const countReq = tx3.objectStore("s").count();
                    countReq.onsuccess = () => {
                        globalThis.__out.count = countReq.result;
                    };
                };
            };
        };
        "#,
    );
    assert!(out.contains(r#""count":1"#), "out={out}");
}
