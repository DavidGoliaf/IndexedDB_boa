//! Appendix D acceptance scenario (TASK-03 §8): versioned schema migration,
//! unique-constraint handling with `preventDefault`, multiEntry indexes, and
//! index-cursor reads with `indexedDB.cmp` ordering.

use boa_engine::{Context, Source};
use boa_idb::extension::IndexedDbExtension;
use boa_idb_core::proto::StorageKey;
use boa_idb_memory::MemoryBackendFactory;
use std::sync::Arc;

fn create_context() -> Context {
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("appendix-d"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .build()
        .expect("Failed to build extension");
    extension
        .register(&mut context)
        .expect("Failed to register extension");
    context
}

#[test]
#[allow(clippy::too_many_lines)]
fn appendix_d_full_scenario() {
    let mut context = create_context();
    context
        .eval(Source::from_bytes(
            r#"
            globalThis.__out = { longueurs: [], asserts: [] };
            function check(cond, name) {
                globalThis.__out.asserts.push(name + "=" + (cond ? "ok" : "FAIL"));
            }
            let req = indexedDB.open("library", 3);
            req.onupgradeneeded = (e) => {
                const db = req.result;
                check(e.oldVersion === 0, "oldVersion-is-0");
                if (e.oldVersion < 1) {
                    const s = db.createObjectStore("books", { keyPath: "isbn" });
                    s.createIndex("by_title", "title", { unique: true });
                    s.createIndex("by_author", "author");
                }
                if (e.oldVersion < 2) {
                    req.transaction.objectStore("books").createIndex("by_year", "year");
                }
                if (e.oldVersion < 3) {
                    const m = db.createObjectStore("magazines", { autoIncrement: true });
                    m.createIndex("by_tags", "tags", { multiEntry: true });
                }
            };
            req.onsuccess = () => {
                const db = req.result;
                check(db.version === 3, "version-is-3");
                const tx = db.transaction(["books", "magazines"], "readwrite");
                const books = tx.objectStore("books");
                books.put({ isbn: 123456, title: "Quarry Memories", author: "Fred", year: 2011 });
                books.put({ isbn: 234567, title: "Water Buffaloes", author: "Fred", year: 2012 });

                const bad = books.add({ isbn: 345678, title: "Quarry Memories", author: "X" });
                bad.onerror = (e) => {
                    check(bad.error && bad.error.name === "ConstraintError", "unique-violation");
                    e.preventDefault();
                };

                tx.objectStore("magazines").put({ tags: ["rust", "db", "rust"] });

                tx.oncomplete = () => {
                    globalThis.__out.txComplete = true;
                    const tx2 = db.transaction("books", "readonly");
                    const idx = tx2.objectStore("books").index("by_author");
                    const out = [];
                    const cursorReq = idx.openCursor(IDBKeyRange.only("Fred"), "next");
                    cursorReq.onsuccess = (ev) => {
                        const c = cursorReq.result;
                        if (c) {
                            out.push(c.primaryKey);
                            c.continue();
                        } else {
                            check(out.length === 2, "two-freds");
                            check(indexedDB.cmp(out[0], out[1]) === -1, "ordered");
                            // Same-handle caching for stores and indexes.
                            const tx3 = db.transaction("books", "readonly");
                            const s1 = tx3.objectStore("books");
                            check(s1 === tx3.objectStore("books"), "same-store");
                            check(s1.index("by_author") === s1.index("by_author"), "same-index");
                            // KeyRange statics.
                            check(IDBKeyRange.only(5).lower === 5, "range-only");
                            check(IDBKeyRange.bound(1, 2).upper === 2, "range-bound");
                            check(IDBKeyRange.lowerBound(1, true).lowerOpen === true, "range-open");
                            check(IDBKeyRange.upperBound(9).upper === 9, "range-upper");
                            check(IDBKeyRange.only(3).includes(3) === true, "range-includes");
                            // Count + getAll over the author index.
                            const tx4 = db.transaction("books", "readonly");
                            const countReq = tx4.objectStore("books").index("by_author").count("Fred");
                            countReq.onsuccess = () => {
                                globalThis.__out.fredCount = countReq.result;
                                const tx5 = db.transaction("books", "readonly");
                                const allReq = tx5.objectStore("books").index("by_author").getAll("Fred");
                                allReq.onsuccess = () => {
                                    globalThis.__out.fredTitles = allReq.result.map((b) => b.title).join(",");
                                };
                            };
                        }
                    };
                };
                tx.onabort = () => { globalThis.__out.txAborted = true; };
            };
            "#,
        ))
        .expect("scenario eval should succeed");
    context.run_jobs().expect("jobs should run");
    context.run_jobs().expect("jobs should run");
    let out = context
        .eval(Source::from_bytes("JSON.stringify(globalThis.__out)"))
        .expect("readback should succeed");
    let out = out
        .to_string(&mut context)
        .expect("to_string")
        .to_std_string_escaped();
    for expected in [
        "oldVersion-is-0=ok",
        "version-is-3=ok",
        "unique-violation=ok",
        "\"txComplete\":true",
        "two-freds=ok",
        "ordered=ok",
        "same-store=ok",
        "same-index=ok",
        "range-only=ok",
        "range-bound=ok",
        "range-open=ok",
        "range-upper=ok",
        "range-includes=ok",
        "\"fredCount\":2",
        "Quarry Memories,Water Buffaloes",
    ] {
        assert!(out.contains(expected), "missing {expected} in {out}");
    }
    assert!(!out.contains("FAIL"), "scenario has failures: {out}");
    assert!(!out.contains("txAborted"), "transaction aborted: {out}");
}
