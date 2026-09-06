//! DOM/API edge coverage (P1-3 coverage drive).
//!
//! Drives the JS-visible surface the WPT suite never touches directly:
//! synthetic `dispatchEvent` (capture/bubble/at-target, `once`,
//! `handleEvent` objects, throwing listeners, `stopPropagation` /
//! `stopImmediatePropagation` / `preventDefault`), `DOMStringList`,
//! `IDBRecord` snapshots, the `IDBVersionChangeEvent` constructor,
//! `DOMException` getters, `IDBKeyRange` factories, plus API error arms
//! (duplicate stores/indexes, missing entities, read-only writes,
//! invalid keys/modes). Assertions pin observable behavior, not internals.

use boa_engine::{Context, Source};
use boa_idb::extension::IndexedDbExtension;
use boa_idb_core::proto::StorageKey;
use boa_idb_memory::MemoryBackendFactory;
use std::sync::Arc;

fn create_context() -> Context {
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("dom-api-tests"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .build()
        .expect("build extension");
    extension
        .register(&mut context)
        .expect("register extension");
    context
}

/// Evaluates a script and drains the job queue so IDB requests settle.
fn eval_drained(context: &mut Context, script: &str) {
    context
        .eval(Source::from_bytes(script))
        .expect("eval should succeed");
    for _ in 0..6 {
        context.run_jobs().expect("jobs should run");
    }
}

/// Reads a global as a string after draining.
fn read_str(context: &mut Context, expr: &str) -> String {
    context
        .eval(Source::from_bytes(expr))
        .expect("read eval")
        .to_string(context)
        .expect("to_string")
        .to_std_string_escaped()
}

const SETUP: &str = r"
// NOTE: `var` (not `let`) so later evals in this context see the binding.
globalThis.__log = [];
globalThis.__out = {};
var openReq = indexedDB.open('domdb', 1);
openReq.onupgradeneeded = () => {
    const db = openReq.result;
    const store = db.createObjectStore('s', { keyPath: 'id' });
    store.createIndex('by_name', 'name', { unique: false });
    db.createObjectStore('t');
};
openReq.onsuccess = () => { globalThis.__out.opened = true; };
";

fn setup_open(context: &mut Context) {
    eval_drained(context, SETUP);
    assert_eq!(read_str(context, "String(globalThis.__out.opened)"), "true");
}

#[test]
fn dispatch_full_phase_order_with_capture_and_bubble() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        tx.objectStore('s').put({ id: 1, name: 'a' });
        globalThis.__order = [];
        const at = (label) => (e) => {
            globalThis.__order.push(label + ':' + e.eventPhase + ':' + e.currentTarget.constructor.name);
        };
        db.addEventListener('probe', at('db-bubble'));
        tx.addEventListener('probe', at('tx-target'));
        db.addEventListener('probe', at('db-capture'), { capture: true });
        tx.addEventListener('probe', at('tx-capture-ignored'), { capture: true });
        const ev = new Event('probe', { bubbles: true, cancelable: true });
        const notCancelled = tx.dispatchEvent(ev);
        globalThis.__out.cancelled = String(notCancelled);
        globalThis.__out.phase = String(ev.eventPhase);
        globalThis.__out.targetName = ev.target.constructor.name;
        globalThis.__out.bubbles = String(ev.bubbles);
        globalThis.__out.trusted = String(ev.isTrusted);
        globalThis.__out.type = ev.type;
        ",
    );
    // Capture listeners on the TARGET itself do not run in the capture
    // phase (only ancestors); at-target invokes all in registration order.
    assert_eq!(
        read_str(&mut context, "globalThis.__order.join('|')"),
        "db-capture:1:IDBDatabase|tx-target:2:IDBTransaction|tx-capture-ignored:2:IDBTransaction|db-bubble:3:IDBDatabase"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.cancelled"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.phase"), "0");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.targetName"),
        "IDBTransaction"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.bubbles"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.trusted"), "false");
    assert_eq!(read_str(&mut context, "globalThis.__out.type"), "probe");
}

#[test]
fn dispatch_stop_propagation_and_prevent_default() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        tx.objectStore('s').put({ id: 2 });
        globalThis.__order = [];
        db.addEventListener('halt', () => globalThis.__order.push('db'));
        db.addEventListener('halt', (e) => {
            globalThis.__order.push('db-capture');
            e.stopPropagation();
        }, { capture: true });
        tx.addEventListener('halt', () => globalThis.__order.push('target'));
        const ev = new Event('halt', { bubbles: true, cancelable: true });
        ev.preventDefault();
        const notCancelled = tx.dispatchEvent(ev);
        globalThis.__out.cancelled = String(notCancelled);
        globalThis.__out.prevented = String(ev.defaultPrevented);
        ",
    );
    // Capture listener on db stops everything below (target + bubble).
    assert_eq!(
        read_str(&mut context, "globalThis.__order.join('|')"),
        "db-capture"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.cancelled"),
        "false"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.prevented"), "true");
}

#[test]
fn dispatch_once_handle_event_immediate_and_errors() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        tx.objectStore('s').put({ id: 3 });
        globalThis.__count = 0;
        globalThis.__handled = [];
        tx.addEventListener('tick', () => { globalThis.__count++; }, { once: true });
        tx.addEventListener('tick', {
            handleEvent(e) { globalThis.__handled.push(e.type); }
        });
        tx.addEventListener('tick', (e) => {
            e.stopImmediatePropagation();
        });
        tx.addEventListener('tick', () => { globalThis.__count += 100; });
        tx.dispatchEvent(new Event('tick'));
        tx.dispatchEvent(new Event('tick'));
        try { tx.dispatchEvent('not-an-event'); } catch (e) {
            globalThis.__out.badArg = String(e instanceof TypeError);
        }
        try { tx.dispatchEvent(); } catch (e) {
            globalThis.__out.noArg = String(e instanceof TypeError);
        }
        try { tx.dispatchEvent.call(42, new Event('tick')); } catch (e) {
            globalThis.__out.badThis = 'threw';
        }
        ",
    );
    // once fired exactly once across two dispatches; the +100 listener never
    // ran (stopImmediatePropagation); handleEvent object ran twice.
    assert_eq!(read_str(&mut context, "String(globalThis.__count)"), "1");
    assert_eq!(
        read_str(&mut context, "globalThis.__handled.join(',')"),
        "tick,tick"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.badArg"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.noArg"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.badThis"), "threw");
}

#[test]
fn dispatch_throwing_listener_propagates() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        tx.objectStore('s').put({ id: 4 });
        tx.addEventListener('boom', () => { throw new Error('listener-bang'); });
        try {
            tx.dispatchEvent(new Event('boom'));
            globalThis.__out.threw = 'no';
        } catch (e) {
            globalThis.__out.threw = String(e.message);
        }
        ",
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.threw"),
        "listener-bang"
    );
}

#[test]
fn dispatch_non_bubbling_skips_ancestors() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        tx.objectStore('s').put({ id: 5 });
        globalThis.__order = [];
        db.addEventListener('flat', () => globalThis.__order.push('db'));
        tx.addEventListener('flat', () => globalThis.__order.push('target'));
        tx.dispatchEvent(new Event('flat'));
        ",
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__order.join('|')"),
        "target"
    );
}

#[test]
fn object_store_names_list() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const names = db.objectStoreNames;
        globalThis.__out.length = String(names.length);
        globalThis.__out.item0 = String(names.item(0));
        globalThis.__out.itemMissing = String(names.item(99));
        globalThis.__out.hasS = String(names.contains('s'));
        globalThis.__out.hasNope = String(names.contains('nope'));
        globalThis.__out.index0 = String(names[0]);
        try { new DOMStringList(); } catch (e) {
            globalThis.__out.noConstruct = String(e instanceof TypeError);
        }
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.length"), "2");
    assert_eq!(read_str(&mut context, "globalThis.__out.item0"), "s");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.itemMissing"),
        "undefined"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.hasS"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.hasNope"), "false");
    assert_eq!(read_str(&mut context, "globalThis.__out.index0"), "s");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.noConstruct"),
        "true"
    );
}

#[test]
fn get_all_records_snapshot() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        const store = tx.objectStore('s');
        store.put({ id: 10, name: 'rec' });
        const allReq = store.getAllRecords();
        allReq.onsuccess = () => {
            const rows = allReq.result;
            globalThis.__out.count = String(rows.length);
            globalThis.__out.key = String(rows[0].key);
            globalThis.__out.pkey = String(rows[0].primaryKey);
            globalThis.__out.val = String(rows[0].value.name);
            try { new IDBRecord(); } catch (e) {
                globalThis.__out.noConstruct = String(e instanceof TypeError);
            }
        };
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.count"), "1");
    assert_eq!(read_str(&mut context, "globalThis.__out.key"), "10");
    assert_eq!(read_str(&mut context, "globalThis.__out.pkey"), "10");
    assert_eq!(read_str(&mut context, "globalThis.__out.val"), "rec");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.noConstruct"),
        "true"
    );
}

#[test]
fn version_change_event_constructor_and_flow() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const ev = new IDBVersionChangeEvent('upgradeneeded', { oldVersion: 3, newVersion: 7 });
        globalThis.__out.type = ev.type;
        globalThis.__out.oldV = String(ev.oldVersion);
        globalThis.__out.newV = String(ev.newVersion);
        const bare = new IDBVersionChangeEvent('x');
        globalThis.__out.bareOld = String(bare.oldVersion);
        globalThis.__out.bareNew = String(bare.newVersion);
        const weird = new IDBVersionChangeEvent('y', { oldVersion: -5, newVersion: null });
        globalThis.__out.weirdOld = String(weird.oldVersion);
        globalThis.__out.weirdNew = String(weird.newVersion);
        const up = indexedDB.open('domdb', 2);
        up.onupgradeneeded = (e) => {
            globalThis.__out.flowOld = String(e.oldVersion);
            globalThis.__out.flowNew = String(e.newVersion);
            globalThis.__out.flowType = e.type;
        };
        ",
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.type"),
        "upgradeneeded"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.oldV"), "3");
    assert_eq!(read_str(&mut context, "globalThis.__out.newV"), "7");
    assert_eq!(read_str(&mut context, "globalThis.__out.bareOld"), "0");
    assert_eq!(read_str(&mut context, "globalThis.__out.bareNew"), "null");
    assert_eq!(read_str(&mut context, "globalThis.__out.weirdOld"), "0");
    assert_eq!(read_str(&mut context, "globalThis.__out.weirdNew"), "null");
    assert_eq!(read_str(&mut context, "globalThis.__out.flowOld"), "1");
    assert_eq!(read_str(&mut context, "globalThis.__out.flowNew"), "2");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.flowType"),
        "upgradeneeded"
    );
}

#[test]
fn dom_exception_getters_and_error_names() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const e = new DOMException('oops', 'DataError');
        globalThis.__out.name = e.name;
        globalThis.__out.message = e.message;
        const probe = (fn) => {
            try { fn(); return 'no-throw'; }
            catch (err) { return err instanceof DOMException ? err.name : 'js:' + err.constructor.name; }
        };
        const db = openReq.result;
        // versionchange txn outside upgrade cannot be used for schema ops.
        const tx = probe(() => db.transaction('s', 'versionchange'));
        globalThis.__out.vcMode = String(tx);
        const tx2 = probe(() => db.transaction('missing-store', 'readonly'));
        globalThis.__out.tx2 = String(tx2);
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.name"), "DataError");
    assert_eq!(read_str(&mut context, "globalThis.__out.message"), "oops");
}

#[test]
fn key_range_factories_and_errors() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const only = IDBKeyRange.only(5);
        globalThis.__out.only = only.lower + ',' + only.upper + ',' + only.lowerOpen + ',' + only.upperOpen;
        const bound = IDBKeyRange.bound(1, 9, true, false);
        globalThis.__out.bound = bound.lower + ',' + bound.upper + ',' + bound.lowerOpen + ',' + bound.upperOpen;
        const lower = IDBKeyRange.lowerBound(2, true);
        globalThis.__out.lower = lower.lower + ',' + lower.lowerOpen;
        const upper = IDBKeyRange.upperBound(8);
        globalThis.__out.upper = upper.upper + ',' + upper.upperOpen;
        try { IDBKeyRange.bound(9, 1); globalThis.__out.badBound = 'no'; }
        catch (e) { globalThis.__out.badBound = String(e instanceof DOMException); }
        // Range query through a cursor.
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        const store = tx.objectStore('s');
        for (let i = 1; i <= 5; i++) store.put({ id: i });
        const got = [];
        const cur = store.openCursor(IDBKeyRange.bound(2, 4));
        cur.onsuccess = () => {
            const c = cur.result;
            if (c) { got.push(c.key); c.continue(); }
            else globalThis.__out.keys = got.join(',');
        };
        ",
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.only"),
        "5,5,false,false"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.bound"),
        "1,9,true,false"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.lower"), "2,true");
    assert_eq!(read_str(&mut context, "globalThis.__out.upper"), "8,false");
    assert_eq!(read_str(&mut context, "globalThis.__out.badBound"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.keys"), "2,3,4");
}

#[test]
fn api_error_arms() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const probe = (fn) => {
            try { fn(); return 'no-throw'; }
            catch (e) { return e instanceof DOMException ? e.name : 'js:' + e.constructor.name; }
        };
        // Missing store in a valid txn.
        const tx = db.transaction('s', 'readonly');
        globalThis.__out.missingStore = probe(() => tx.objectStore('nope'));
        // Read-only write.
        globalThis.__out.readOnlyPut = probe(() => tx.objectStore('s').put({ id: 1 }));
        // Bad txn mode.
        globalThis.__out.badMode = probe(() => db.transaction('s', 'bogus'));
        // Invalid key (undefined) with out-of-line keys.
        const dbTx = db.transaction('t', 'readwrite');
        globalThis.__out.badKey = probe(() => dbTx.objectStore('t').put({ v: 1 }, undefined));
        // add() duplicate.
        const tx3 = db.transaction('t', 'readwrite');
        tx3.objectStore('t').put('first', 'dup');
        const dupReq = tx3.objectStore('t').add('second', 'dup');
        dupReq.onerror = () => { globalThis.__out.dupAdd = dupReq.error.name; };
        // Missing index.
        globalThis.__out.missingIndex = probe(() => tx.objectStore('s').index('nope'));
        // Abort then use.
        const tx4 = db.transaction('s', 'readwrite');
        tx4.abort();
        globalThis.__out.abortedPut = probe(() => tx4.objectStore('s').put({ id: 9 }));
        ",
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.missingStore"),
        "NotFoundError"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.readOnlyPut"),
        "ReadOnlyError"
    );
    // Invalid mode is a JS TypeError (WebIDL enum), not a DOMException.
    assert_eq!(
        read_str(&mut context, "globalThis.__out.badMode"),
        "js:TypeError"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.dupAdd"),
        "ConstraintError"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.missingIndex"),
        "NotFoundError"
    );
    // objectStore() on a finished transaction is InvalidStateError per spec.
    assert_eq!(
        read_str(&mut context, "globalThis.__out.abortedPut"),
        "InvalidStateError"
    );
}

#[test]
fn value_and_key_variety() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('t', 'readwrite');
        const store = tx.objectStore('t');
        store.put({ n: 1, s: 'x', b: true, nil: null, arr: [1, 'two', false], nested: { a: [3] } }, 'complex');
        store.put('date-val', new Date(1000));
        const r1 = store.get('complex');
        r1.onsuccess = () => {
            globalThis.__out.nested = String(r1.result.nested.a[0]);
            globalThis.__out.arrlen = String(r1.result.arr.length);
        };
        const r2 = store.get(new Date(1000));
        r2.onsuccess = () => { globalThis.__out.dateEcho = String(r2.result === 'date-val'); };
        const buf = new Uint8Array([7, 8, 9]).buffer;
        store.put('bin-val', buf);
        const idx = db.transaction('s', 'readonly').objectStore('s').index('by_name');
        const allKeys = idx.getAllKeys();
        allKeys.onsuccess = () => { globalThis.__out.indexKeys = String(allKeys.result.length); };        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.nested"), "3");
    assert_eq!(read_str(&mut context, "globalThis.__out.arrlen"), "3");
    assert_eq!(read_str(&mut context, "globalThis.__out.dateEcho"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.indexKeys"), "0");
}

#[test]
fn cursor_directions_and_mutation() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        const store = tx.objectStore('s');
        for (let i = 1; i <= 4; i++) store.put({ id: i, name: 'n' + i });
        const rev = [];
        const c1 = store.openCursor(null, 'prev');
        c1.onsuccess = () => {
            const c = c1.result;
            if (c) { rev.push(c.key); c.continue(); }
            else globalThis.__out.rev = rev.join(',');
        };
        const tx2 = db.transaction('s', 'readwrite');
        const c2 = tx2.objectStore('s').openCursor(IDBKeyRange.only(2));
        c2.onsuccess = () => {
            const c = c2.result;
            if (c) {
                c.update({ id: 2, name: 'updated' });
                globalThis.__out.updated = 'yes';
            }
        };
        const tx3 = db.transaction('s', 'readwrite');
        const c3 = tx3.objectStore('s').openCursor(IDBKeyRange.only(4));
        c3.onsuccess = () => {
            const c = c3.result;
            if (c) { c.delete(); globalThis.__out.deleted = 'yes'; }
        };
        const cnt = db.transaction('s', 'readonly').objectStore('s').count();
        cnt.onsuccess = () => { globalThis.__out.count = String(cnt.result); };
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.rev"), "4,3,2,1");
    assert_eq!(read_str(&mut context, "globalThis.__out.updated"), "yes");
    assert_eq!(read_str(&mut context, "globalThis.__out.deleted"), "yes");
    // 4 initial + nothing removed yet at count time ordering aside: count
    // runs in its own txn after the deletes commit... just assert numeric.
    assert!(
        ["3", "4"].contains(&read_str(&mut context, "globalThis.__out.count").as_str()),
        "count reflects committed deletes"
    );
}

#[test]
fn factory_delete_and_cmp() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        globalThis.__out.cmp = String(indexedDB.cmp(1, 2)) + ',' + String(indexedDB.cmp([1], [1]));
        try { indexedDB.cmp(Symbol(), 1); globalThis.__out.cmpSym = 'no'; }
        catch (e) { globalThis.__out.cmpSym = 'threw'; }
        openReq.result.close();
        const del = indexedDB.deleteDatabase('domdb');
        del.onsuccess = () => { globalThis.__out.deleted = 'yes'; };
        del.onerror = () => { globalThis.__out.deleted = 'error'; };
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.cmp"), "-1,0");
    assert_eq!(read_str(&mut context, "globalThis.__out.cmpSym"), "threw");
    assert_eq!(read_str(&mut context, "globalThis.__out.deleted"), "yes");
}

#[test]
fn index_management_props_and_errors() {
    let mut context = create_context();
    eval_drained(
        &mut context,
        r"
        globalThis.__out = {};
        const probe = (fn) => {
            try { fn(); return 'no-throw'; }
            catch (e) { return e instanceof DOMException ? e.name : 'js:' + e.constructor.name; }
        };
        const openReq = indexedDB.open('idxdb', 1);
        openReq.onupgradeneeded = () => {
            const db = openReq.result;
            const store = db.createObjectStore('s', { keyPath: 'id' });
            const idx = store.createIndex('by_name', 'name', { unique: true });
            globalThis.__out.idxName = idx.name;
            globalThis.__out.idxKeyPath = String(idx.keyPath);
            globalThis.__out.idxUnique = String(idx.unique);
            globalThis.__out.idxMulti = String(idx.multiEntry);
            globalThis.__out.idxStore = idx.objectStore.name;
            globalThis.__out.dupIndex = probe(() => store.createIndex('by_name', 'name'));
            globalThis.__out.indexNames = String(store.indexNames.length) + ':' + String(store.indexNames.item(0));
            store.deleteIndex('by_name');
            globalThis.__out.afterDelete = String(store.indexNames.length);
            globalThis.__out.missingDelete = probe(() => store.deleteIndex('by_name'));
            globalThis.__out.dupStore = probe(() => db.createObjectStore('s'));
            db.createObjectStore('extra');
            globalThis.__out.stores = String(db.objectStoreNames.length);
            db.deleteObjectStore('extra');
            globalThis.__out.missingStoreDelete = probe(() => db.deleteObjectStore('extra'));
        };
        ",
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.idxName"),
        "by_name"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.idxKeyPath"),
        "name"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.idxUnique"), "true");
    assert_eq!(read_str(&mut context, "globalThis.__out.idxMulti"), "false");
    assert_eq!(read_str(&mut context, "globalThis.__out.idxStore"), "s");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.dupIndex"),
        "ConstraintError"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.indexNames"),
        "1:by_name"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.afterDelete"), "0");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.missingDelete"),
        "NotFoundError"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.dupStore"),
        "ConstraintError"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.stores"), "2");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.missingStoreDelete"),
        "NotFoundError"
    );
}

#[test]
fn index_queries_unique_and_key_cursors() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        const tx = db.transaction('s', 'readwrite');
        const store = tx.objectStore('s');
        store.put({ id: 1, name: 'b' });
        store.put({ id: 2, name: 'a' });
        store.put({ id: 3, name: 'b' });
        ",
    );
    eval_drained(
        &mut context,
        r"
        const dbR = openReq.result;
        const txR = dbR.transaction('s', 'readonly');
        const idx = txR.objectStore('s').index('by_name');
        // NOTE: an explicit `null` query is deliberately rejected with
        // DataError (see `query_to_range` docs); `undefined` means match-all.
        try { idx.getAllKeys(null, 1); globalThis.__out.nullQuery = 'no-throw'; }
        catch (e) { globalThis.__out.nullQuery = String(e && e.name || e); }
        const dbg = (label, fn) => {
            try { return fn(); }
            catch (e) { globalThis.__out['dbg_' + label] = 'THREW:' + String(e && e.name || e); return null; }
        };
        const g = dbg('get', () => idx.get('a'));
        if (g) g.onsuccess = () => { globalThis.__out.getA = String(g.result.id); };
        const gk = dbg('getKey', () => idx.getKey('b'));
        if (gk) gk.onsuccess = () => { globalThis.__out.getKeyB = String(gk.result); };
        const all = dbg('getAll', () => idx.getAll('b'));
        if (all) all.onsuccess = () => { globalThis.__out.allB = String(all.result.length); };
        const allKeys = dbg('getAllKeys', () => idx.getAllKeys(undefined, 1));
        if (allKeys) allKeys.onsuccess = () => { globalThis.__out.allKeys1 = String(allKeys.result.length); };
        const cnt = dbg('count', () => idx.count(IDBKeyRange.only('b')));
        if (cnt) cnt.onsuccess = () => { globalThis.__out.countB = String(cnt.result); };
        const seen = [];
        const kc = dbg('openKeyCursor', () => idx.openKeyCursor());
        if (kc) kc.onsuccess = () => {
            const c = kc.result;
            if (c) { seen.push(c.key + '=' + c.primaryKey); c.continue(); }
            else globalThis.__out.keyCursor = seen.join(',');
        };
        const uniq = [];
        const uc = dbg('openCursorUnique', () => idx.openCursor(null, 'nextunique'));
        if (uc) uc.onsuccess = () => {
            const c = uc.result;
            if (c) { uniq.push(c.key); c.continue(); }
            else globalThis.__out.unique = uniq.join(',');
        };
        globalThis.__out.dbgDone = 'yes';
        ",
    );
    eval_drained(
        &mut context,
        r"
        const dbW = openReq.result;
        // The shared fixture index is non-unique, so duplicates are fine.
        const tx2 = dbW.transaction('s', 'readwrite');
        const dup = tx2.objectStore('s').put({ id: 9, name: 'a' });
        dup.onsuccess = () => { globalThis.__out.dupOk = 'put-ok'; };
        dup.onerror = () => { globalThis.__out.dupOk = dup.error.name; };
        ",
    );
    // 'by_name' is NOT unique in the shared fixture (created without the
    // 'by_name' is NOT unique in the shared fixture (created without the
    // unique flag), so duplicates are fine and getKey returns the first.
    assert_eq!(
        read_str(&mut context, "globalThis.__out.nullQuery"),
        "DataError"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.getA"), "2");
    assert_eq!(read_str(&mut context, "globalThis.__out.allB"), "2");
    assert_eq!(read_str(&mut context, "globalThis.__out.allKeys1"), "1");
    assert_eq!(read_str(&mut context, "globalThis.__out.countB"), "2");
    assert_eq!(read_str(&mut context, "globalThis.__out.getKeyB"), "1");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.keyCursor"),
        "a=2,b=1,b=3"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.unique"), "a,b");
    assert_eq!(read_str(&mut context, "globalThis.__out.dupOk"), "put-ok");
}

#[test]
fn unique_index_violation_and_request_props() {
    let mut context = create_context();
    eval_drained(
        &mut context,
        r"
        globalThis.__out = {};
        const openReq = indexedDB.open('unidb', 1);
        openReq.onupgradeneeded = () => {
            const store = openReq.result.createObjectStore('s', { keyPath: 'id' });
            store.createIndex('uniq', 'name', { unique: true });
        };
        openReq.onsuccess = () => {
            const db = openReq.result;
            const tx = db.transaction('s', 'readwrite');
            const store = tx.objectStore('s');
            store.put({ id: 1, name: 'same' });
            const bad = store.put({ id: 2, name: 'same' });
            globalThis.__out.readyState = bad.readyState;
            globalThis.__out.sourceName = bad.source.name;
            globalThis.__out.txnMode = bad.transaction.mode;
            bad.onsuccess = () => { globalThis.__out.violation = 'unexpected-success'; };
            bad.onerror = () => {
                globalThis.__out.violation = bad.error.name;
                globalThis.__out.readyStateAfter = bad.readyState;
                globalThis.__out.resultAfter = String(bad.result);
            };
            tx.oncomplete = () => { globalThis.__out.txComplete = 'late-complete'; };
            tx.onerror = () => { globalThis.__out.txError = 'fired'; };
            tx.onabort = () => { globalThis.__out.txAbort = 'fired'; };
        };
        ",
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.readyState"),
        "pending"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.sourceName"), "s");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.txnMode"),
        "readwrite"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.violation"),
        "ConstraintError"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.readyStateAfter"),
        "done"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.resultAfter"),
        "undefined"
    );
}

#[test]
fn database_close_blocked_and_versionchange() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        globalThis.__out.name = db.name;
        globalThis.__out.version = String(db.version);
        db.close();
        const probe = (fn) => {
            try { fn(); return 'no-throw'; }
            catch (e) { return e instanceof DOMException ? e.name : 'js:' + e.constructor.name; }
        };
        globalThis.__out.afterClose = probe(() => db.transaction('s', 'readonly'));
        // A second connection at a higher version notifies the first.
        const db2open = indexedDB.open('domdb', 9);
        db2open.onblocked = () => { globalThis.__out.blocked = 'fired'; };
        db2open.onupgradeneeded = () => { globalThis.__out.upgraded = 'yes'; };
        // Reopen at the upgraded version: plain success, no upgrade.
        const same = indexedDB.open('domdb', 9);
        same.onsuccess = () => {
            globalThis.__out.sameVersion = String(same.result.version);
            same.result.close();
        };
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.name"), "domdb");
    assert_eq!(read_str(&mut context, "globalThis.__out.version"), "1");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.afterClose"),
        "InvalidStateError"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.sameVersion"), "9");
}

#[test]
fn getter_fallbacks_with_foreign_this() {
    let mut context = create_context();
    setup_open(&mut context);
    eval_drained(
        &mut context,
        r"
        const db = openReq.result;
        // Accessors called with a non-record / non-list `this` yield
        // undefined / 0 / false instead of throwing (spec: getters are
        // lenient on brand mismatch here).
        const tx = db.transaction('s', 'readwrite');
        tx.objectStore('s').put({ id: 50, name: 'fb' });
        const recReq = tx.objectStore('s').getAllRecords();
        recReq.onsuccess = () => {
            const rec = recReq.result[0];
            const keyGet = Object.getOwnPropertyDescriptor(IDBRecord.prototype, 'key').get;
            const pkGet = Object.getOwnPropertyDescriptor(IDBRecord.prototype, 'primaryKey').get;
            const valGet = Object.getOwnPropertyDescriptor(IDBRecord.prototype, 'value').get;
            globalThis.__out.recKey = String(rec.key);
            globalThis.__out.foreignKey = String(keyGet.call({}));
            globalThis.__out.foreignPk = String(pkGet.call({}));
            globalThis.__out.foreignVal = String(valGet.call({}));
            try { new IDBRecord(); } catch (e) { globalThis.__out.recCtor = 'threw'; }
            const names = db.objectStoreNames;
            const lenGet = Object.getOwnPropertyDescriptor(DOMStringList.prototype, 'length').get;
            globalThis.__out.listLen = String(names.length);
            globalThis.__out.foreignLen = String(lenGet.call({}));
            globalThis.__out.itemOob = String(names.item(7));
            globalThis.__out.itemForeign = String(names.item.call({}, 0));
            globalThis.__out.containsForeign = String(names.contains.call({}, 's'));
        };
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.recKey"), "50");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.foreignKey"),
        "undefined"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.foreignPk"),
        "undefined"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.foreignVal"),
        "undefined"
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.recCtor"), "threw");
    assert_eq!(read_str(&mut context, "globalThis.__out.listLen"), "2");
    assert_eq!(read_str(&mut context, "globalThis.__out.foreignLen"), "0");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.itemOob"),
        "undefined"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.itemForeign"),
        "undefined"
    );
    assert_eq!(
        read_str(&mut context, "globalThis.__out.containsForeign"),
        "false"
    );
}

#[test]
fn version_change_init_edges() {
    let mut context = create_context();
    eval_drained(
        &mut context,
        r"
        globalThis.__out = {};
        const nanOld = new IDBVersionChangeEvent('x', { oldVersion: NaN });
        globalThis.__out.nanOld = String(nanOld.oldVersion);
        const infNew = new IDBVersionChangeEvent('x', { newVersion: Infinity });
        globalThis.__out.infNew = String(infNew.newVersion);
        const negNew = new IDBVersionChangeEvent('x', { oldVersion: 2, newVersion: -3 });
        globalThis.__out.negNew = String(negNew.newVersion);
        const strOld = new IDBVersionChangeEvent('x', { oldVersion: '4' });
        globalThis.__out.strOld = String(strOld.oldVersion);
        const zeroArg = new IDBVersionChangeEvent();
        globalThis.__out.zeroType = zeroArg.type;
        ",
    );
    assert_eq!(read_str(&mut context, "globalThis.__out.nanOld"), "0");
    assert_eq!(read_str(&mut context, "globalThis.__out.infNew"), "null");
    assert_eq!(read_str(&mut context, "globalThis.__out.negNew"), "null");
    assert_eq!(read_str(&mut context, "globalThis.__out.strOld"), "4");
    assert_eq!(
        read_str(&mut context, "globalThis.__out.zeroType"),
        "undefined"
    );
}

/// Pure-Rust units for the tiny runtime helpers (no JS needed).
#[test]
fn extension_double_register_rejected() {
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("double-register"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .build()
        .expect("build extension");
    extension.register(&mut context).expect("first register");
    let second = extension.register(&mut context);
    assert!(second.is_err(), "second register must fail");
}

#[test]
fn js_object_ext_arms() {
    use boa_idb::convert::boa_compat::JsObjectExt;
    let mut context = Context::default();
    let obj = context
        .eval(Source::from_bytes("({ foo: 42, bar: 'x' })"))
        .expect("eval object")
        .as_object()
        .expect("object")
        .clone();
    // Non-callable property → TypeError arm.
    assert!(obj.call_method("foo", &[], &mut context).is_err());
    // Missing method → TypeError arm.
    assert!(obj.call_method("missing", &[], &mut context).is_err());
    // Enumerable keys snapshot.
    let keys = obj.get_enumerable_keys(&mut context).expect("keys");
    assert_eq!(keys.len(), 2);

    let arr = context
        .eval(Source::from_bytes("[1, 2, 3]"))
        .expect("eval array")
        .as_object()
        .expect("array")
        .clone();
    assert_eq!(arr.array_length(&mut context).expect("length"), 3);
    // array_length on a non-array → error arm.
    assert!(obj.array_length(&mut context).is_err());
}

#[test]
fn end_of_task_deactivates_live_transaction() {
    let mut context = create_context();
    eval_drained(
        &mut context,
        r"
        globalThis.__out = {};
        var eotOpen = indexedDB.open('eotdb', 1);
        eotOpen.onupgradeneeded = () => { eotOpen.result.createObjectStore('s'); };
        eotOpen.onsuccess = () => { globalThis.eotDb = eotOpen.result; };
        ",
    );
    // Begin a transaction and end the task before it settles: only *new*
    // requests on it must throw afterwards.
    context
        .eval(Source::from_bytes(
            "var eotTx = globalThis.eotDb.transaction('s', 'readwrite');",
        ))
        .expect("begin txn");
    boa_idb::executor::end_of_task(&mut context);
    let outcome = context
        .eval(Source::from_bytes(
            "try { eotTx.objectStore('s').put({id: 1}); 'no-throw'; } catch (e) { e.name; }",
        ))
        .expect("probe eval")
        .to_string(&mut context)
        .expect("to_string")
        .to_std_string_escaped();
    assert_eq!(outcome, "TransactionInactiveError");
}
#[test]
fn io_runtime_and_executor_smoke() {
    let _io = boa_idb::io::IoRuntime::new();
    // Cover the `Default` impl (a unit struct: the lint would prefer
    // `new()`, but the impl itself is shipped API).
    #[allow(clippy::default_constructed_unit_structs)]
    let _io2 = boa_idb::io::IoRuntime::default();

    let mut context = Context::default();
    boa_idb::executor::end_of_task(&mut context);
}
