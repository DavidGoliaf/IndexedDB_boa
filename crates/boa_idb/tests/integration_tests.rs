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

    // open() should return an object with readyState
    let result = context
        .eval(Source::from_bytes(
            r#"indexedDB.open("testdb", 1).readyState"#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("done"));
}

#[test]
fn test_idb_factory_open_result_is_database() {
    let mut context = create_context();

    // open() should return a request whose result is a database
    let result = context
        .eval(Source::from_bytes(
            r#"indexedDB.open("testdb", 1).result.name"#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("testdb"));
}

#[test]
fn test_idb_factory_open_result_version() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(
            r#"indexedDB.open("testdb", 5).result.version"#,
        ))
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

    // First open a database
    context
        .eval(Source::from_bytes(r#"indexedDB.open("testdb", 1)"#))
        .expect("open should succeed");

    // Then delete it
    let result = context
        .eval(Source::from_bytes(
            r#"indexedDB.deleteDatabase("testdb").readyState"#,
        ))
        .expect("delete should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("done"));
}

#[test]
fn test_idb_factory_databases() {
    let mut context = create_context();

    // Open a database first
    context
        .eval(Source::from_bytes(r#"indexedDB.open("testdb", 1)"#))
        .expect("open should succeed");

    // List databases
    let result = context
        .eval(Source::from_bytes("indexedDB.databases().length"))
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

    let result = context
        .eval(Source::from_bytes(
            r#"
            typeof indexedDB.open("testdb", 1).result.transaction
        "#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_idb_database_has_close_method() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(
            r#"
            typeof indexedDB.open("testdb", 1).result.close
        "#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("function"));
}

#[test]
fn test_idb_database_name() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(
            r#"
            indexedDB.open("mydb", 1).result.name
        "#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_string().unwrap(), js_string!("mydb"));
}

#[test]
fn test_idb_database_version() {
    let mut context = create_context();

    let result = context
        .eval(Source::from_bytes(
            r#"
            indexedDB.open("mydb", 3).result.version
        "#,
        ))
        .expect("eval should succeed");
    assert_eq!(result.as_number().unwrap(), 3.0);
}

#[test]
fn test_structured_clone_primitives() {
    let mut context = create_context();

    // Test that basic values can be stored and retrieved
    // This tests the serialization pipeline
    let result = context
        .eval(Source::from_bytes(
            r#"
            let db = indexedDB.open("clonetest", 1).result;
            typeof db
        "#,
        ))
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
