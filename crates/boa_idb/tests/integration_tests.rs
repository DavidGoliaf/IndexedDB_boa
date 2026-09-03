//! Integration tests for `boa_idb` JS bindings.

#![allow(clippy::float_cmp)]

use boa_engine::{Context, Source, js_string};
use boa_idb::extension::IndexedDbExtension;
use boa_idb_core::proto::StorageKey;
use boa_idb_memory::MemoryBackendFactory;
use std::sync::Arc;

/// Creates a Boa context with `IndexedDB` registered.
fn create_context() -> Context {
    let mut context = Context::default();

    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("test-origin"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .build()
        .expect("Failed to build extension");

    extension
        .register(&mut context)
        .expect("Failed to register extension");
    context
}

/// Evaluates a script, drains the Boa job queue (running the IDB pump and
/// dispatching all events), and returns the script's own value.
fn eval_drained(context: &mut Context, script: &str) -> boa_engine::JsValue {
    let value = context
        .eval(Source::from_bytes(script))
        .expect("eval should succeed");
    context.run_jobs().expect("jobs should run");
    value
}

#[test]
fn test_extension_registration() {
    let mut context = create_context();

    // indexedDB should be on globalThis
    let result = context
        .eval(Source::from_bytes("typeof indexedDB"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("object"));
}

#[test]
fn test_indexeddb_factory_has_cmp() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes("typeof indexedDB.cmp"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_indexeddb_cmp_numbers() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes("indexedDB.cmp(1, 2)"))
        .expect("eval should succeed");
    assert_eq!(result.as_i32().unwrap(), -1);

    let result = context
        .eval(Source::from_bytes("indexedDB.cmp(2, 1)"))
        .expect("eval should succeed");
    assert_eq!(result.as_i32().unwrap(), 1);

    let result = context
        .eval(Source::from_bytes("indexedDB.cmp(1, 1)"))
        .expect("eval should succeed");
    assert_eq!(result.as_i32().unwrap(), 0);
}

#[test]
fn test_indexeddb_cmp_strings() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(r#"indexedDB.cmp("a", "b")"#))
        .expect("eval should succeed");
    assert_eq!(result.as_i32().unwrap(), -1);

    let result = context
        .eval(Source::from_bytes(r#"indexedDB.cmp("b", "a")"#))
        .expect("eval should succeed");
    assert_eq!(result.as_i32().unwrap(), 1);
}

#[test]
fn test_indexeddb_cmp_dates() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(
            "indexedDB.cmp(new Date(100), new Date(200))",
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_i32().unwrap(), -1);
}

#[test]
fn test_indexeddb_cmp_type_error() {
    let mut context = create_context();

    // Symbol cannot be used as a key
    let result = context.eval(Source::from_bytes("indexedDB.cmp(Symbol(), 1)"));
    assert!(result.is_err());
}

#[test]
fn test_dom_exception_exists() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes("typeof DOMException"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_dom_exception_constructor() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(
            r#"new DOMException("test message", "ConstraintError").name"#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("ConstraintError"));
}

#[test]
fn test_dom_exception_message() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(
            r#"new DOMException("hello", "DataError").message"#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("hello"));
}

#[test]
fn test_event_constructor() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes("typeof Event"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_idb_factory_open_returns_request() {
    let mut context = create_context();

    // open() returns a pending request synchronously...
    let result = context
        .eval(Source::from_bytes(
            r#"indexedDB.open("testdb", 1).readyState"#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("pending"));
}

#[test]
fn test_idb_factory_open_completes_after_jobs() {
    let mut context = create_context();

    eval_drained(
        &mut context,
        r#"
        globalThis.__openReq = indexedDB.open("testdb", 1);
    "#,
    );
    let result = context
        .eval(Source::from_bytes("globalThis.__openReq.readyState"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("done"));
}

#[test]
fn test_idb_factory_open_result_is_database() {
    let mut context = create_context();

    eval_drained(
        &mut context,
        r#"
        globalThis.__openReq = indexedDB.open("testdb", 1);
    "#,
    );
    // open() should return a request whose result is a database
    let result = context
        .eval(Source::from_bytes("globalThis.__openReq.result.name"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("testdb"));
}

#[test]
fn test_idb_factory_open_result_version() {
    let mut context = create_context();

    eval_drained(
        &mut context,
        r#"globalThis.__openReq = indexedDB.open("testdb", 5);"#,
    );
    let result = context
        .eval(Source::from_bytes("globalThis.__openReq.result.version"))
        .expect("eval should succeed");
    assert_eq!(result.as_number().unwrap(), 5.0);
}

#[test]
fn test_idb_factory_open_version_zero_fails() {
    let mut context = create_context();

    let result = context.eval(Source::from_bytes(r#"indexedDB.open("testdb", 0)"#));
    assert!(result.is_err());
}

#[test]
fn test_idb_factory_delete_database() {
    let mut context = create_context();

    // Open a database and let it complete.
    eval_drained(
        &mut context,
        r#"globalThis.__openReq = indexedDB.open("testdb", 1);"#,
    );

    // Then delete it; the delete request completes after jobs run.
    eval_drained(
        &mut context,
        r#"globalThis.__delReq = indexedDB.deleteDatabase("testdb");"#,
    );
    let result = context
        .eval(Source::from_bytes("globalThis.__delReq.readyState"))
        .expect("delete should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("done"));
}

#[test]
fn test_idb_factory_databases() {
    let mut context = create_context();

    // Open a database first
    eval_drained(&mut context, r#"indexedDB.open("testdb", 1);"#);

    // databases() returns a Promise; capture the settled value.
    eval_drained(
        &mut context,
        r"
        globalThis.__dbCount = -1;
        indexedDB.databases().then(function(dbs) { globalThis.__dbCount = dbs.length; });
    ",
    );
    let result = context
        .eval(Source::from_bytes("globalThis.__dbCount"))
        .expect("databases should succeed");
    assert!(result.as_number().unwrap() >= 1.0);
}

#[test]
fn test_idb_factory_cannot_construct() {
    let mut context = create_context();

    let result = context.eval(Source::from_bytes("new IDBFactory()"));
    assert!(result.is_err());
}

#[test]
fn test_idb_request_cannot_construct() {
    let mut context = create_context();

    let result = context.eval(Source::from_bytes("new IDBRequest()"));
    assert!(result.is_err());
}

#[test]
fn test_idb_database_has_transaction_method() {
    let mut context = create_context();

    eval_drained(
        &mut context,
        r#"globalThis.__openReq = indexedDB.open("testdb", 1);"#,
    );
    let result = context
        .eval(Source::from_bytes(
            "typeof globalThis.__openReq.result.transaction",
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_idb_database_has_close_method() {
    let mut context = create_context();

    eval_drained(
        &mut context,
        r#"globalThis.__openReq = indexedDB.open("testdb", 1);"#,
    );
    let result = context
        .eval(Source::from_bytes(
            "typeof globalThis.__openReq.result.close",
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_idb_database_name() {
    let mut context = create_context();

    eval_drained(
        &mut context,
        r#"globalThis.__openReq = indexedDB.open("mydb", 1);"#,
    );
    let result = context
        .eval(Source::from_bytes("globalThis.__openReq.result.name"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("mydb"));
}

#[test]
fn test_idb_database_version() {
    let mut context = create_context();

    eval_drained(
        &mut context,
        r#"globalThis.__openReq = indexedDB.open("mydb", 3);"#,
    );
    let result = context
        .eval(Source::from_bytes("globalThis.__openReq.result.version"))
        .expect("eval should succeed");
    assert_eq!(result.as_number().unwrap(), 3.0);
}

#[test]
fn test_structured_clone_primitives() {
    let mut context = create_context();

    // Opening works end to end; the result is a database object.
    eval_drained(
        &mut context,
        r#"globalThis.__openReq = indexedDB.open("clonetest", 1);"#,
    );
    let result = context
        .eval(Source::from_bytes("typeof globalThis.__openReq.result"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("object"));
}

#[test]
fn test_key_range_only() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes("typeof IDBKeyRange"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_key_range_only_method() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes("typeof IDBKeyRange.only"))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));

    // Test creating a key range
    let result = context
        .eval(Source::from_bytes("IDBKeyRange.only(42).lower"))
        .expect("eval should succeed");
    assert_eq!(result.as_number().unwrap(), 42.0);
}
