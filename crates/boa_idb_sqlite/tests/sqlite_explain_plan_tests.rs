//! EXPLAIN QUERY PLAN tests over the *real* cursor SQL.
//!
//! Each test builds the exact query shape [`SqliteCursor`] executes (via the
//! [`debug_sql_with_literals`][boa_idb_sqlite::cursor::SqliteCursor::debug_sql_with_literals]
//! hook, bounds inlined as literals) and asserts the plan contains no table
//! scan. This guards the "no `SCAN TABLE`" requirement against query drift:
//! hand-written SQL would not catch regressions in the builder.

use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::proto::{Direction, SourceRef};
use boa_idb_sqlite::cursor::{CursorKindRepr, SqliteCursor, debug_count_sql_with_literals};
use rusqlite::Connection;

fn ranged() -> EncodedRange {
    EncodedRange::bound(b"k00010".to_vec(), false, b"k00050".to_vec(), false)
}

fn open_bound() -> EncodedRange {
    EncodedRange::bound(b"k00010".to_vec(), true, b"k00050".to_vec(), true)
}

/// Real schema (tables + indexes) on an empty database: plans are structural.
fn schema_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE records(store_id INTEGER NOT NULL, key BLOB NOT NULL, \
         value BLOB, ext TEXT, vlen INTEGER NOT NULL, \
         PRIMARY KEY (store_id, key)) WITHOUT ROWID;
         CREATE TABLE index_records(index_id INTEGER NOT NULL, key BLOB NOT NULL, \
         pkey BLOB NOT NULL, PRIMARY KEY (index_id, key, pkey)) WITHOUT ROWID;
         CREATE INDEX idx_index_records_pkey ON index_records(index_id, pkey);",
    )
    .unwrap();
    conn
}

/// Runs `EXPLAIN QUERY PLAN` and returns detail lines.
fn explain_plan(conn: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
    stmt.query_map([], |row| {
        let detail: String = row.get(3)?;
        Ok(detail)
    })
    .unwrap()
    .map(|r| r.unwrap())
    .collect()
}

/// Asserts no full table scan. `SCAN <subquery>` (materialized GROUP BY over
/// an index-bounded range) is not a table scan and is allowed.
fn assert_no_table_scan(plan: &[String], query_desc: &str) {
    let joined = plan.join("\n");
    assert!(
        !joined.contains("SCAN TABLE"),
        "Query plan for '{query_desc}' contains SCAN TABLE:\n{joined}"
    );
}

#[test]
fn test_store_cursor_all_directions_no_scan() {
    let conn = schema_conn();
    for dir in [
        Direction::Next,
        Direction::NextUnique,
        Direction::Prev,
        Direction::PrevUnique,
    ] {
        for key_only in [false, true] {
            let sql = SqliteCursor::debug_sql_with_literals(
                CursorKindRepr::Store,
                &ranged(),
                dir,
                key_only,
                None,
            );
            let plan = explain_plan(&conn, &sql);
            assert_no_table_scan(&plan, &format!("store {dir:?} key_only={key_only}"));
        }
    }
}

#[test]
fn test_store_cursor_open_bounds_and_unbounded_no_scan() {
    let conn = schema_conn();
    for range in [EncodedRange::all(), open_bound()] {
        let sql = SqliteCursor::debug_sql_with_literals(
            CursorKindRepr::Store,
            &range,
            Direction::Next,
            false,
            None,
        );
        let plan = explain_plan(&conn, &sql);
        assert_no_table_scan(&plan, "store open/unbounded");
    }
}

#[test]
fn test_index_cursor_all_directions_no_scan() {
    let conn = schema_conn();
    for (kind, name) in [
        (CursorKindRepr::IndexPlain, "plain"),
        (CursorKindRepr::IndexUnique, "unique"),
    ] {
        for dir in [
            Direction::Next,
            Direction::NextUnique,
            Direction::Prev,
            Direction::PrevUnique,
        ] {
            for key_only in [false, true] {
                let sql =
                    SqliteCursor::debug_sql_with_literals(kind, &ranged(), dir, key_only, None);
                let plan = explain_plan(&conn, &sql);
                assert_no_table_scan(&plan, &format!("index {name} {dir:?} key_only={key_only}"));
            }
        }
    }
}

#[test]
fn test_index_cursor_with_seek_no_scan() {
    let conn = schema_conn();
    // Direction-aware seek predicates must stay index-served too.
    let sql = SqliteCursor::debug_sql_with_literals(
        CursorKindRepr::IndexPlain,
        &ranged(),
        Direction::Next,
        false,
        Some((b"k00020".to_vec(), Some(b"p00001".to_vec()))),
    );
    let plan = explain_plan(&conn, &sql);
    assert_no_table_scan(&plan, "index seek next");

    let sql = SqliteCursor::debug_sql_with_literals(
        CursorKindRepr::IndexPlain,
        &ranged(),
        Direction::Prev,
        false,
        Some((b"k00020".to_vec(), None)),
    );
    let plan = explain_plan(&conn, &sql);
    assert_no_table_scan(&plan, "index seek prev");

    let sql = SqliteCursor::debug_sql_with_literals(
        CursorKindRepr::IndexUnique,
        &ranged(),
        Direction::NextUnique,
        false,
        Some((b"k00020".to_vec(), None)),
    );
    let plan = explain_plan(&conn, &sql);
    assert_no_table_scan(&plan, "unique seek");
}

/// Count queries (`SqliteTxn::count`) share the cursor's range predicate
/// compiler and must stay index-served for both sources.
#[test]
fn test_count_store_and_index_no_scan() {
    let conn = schema_conn();
    for (src, desc) in [
        (SourceRef::Store(1), "count store"),
        (SourceRef::Index { store: 1, index: 1 }, "count index"),
    ] {
        for range in [EncodedRange::all(), ranged(), open_bound()] {
            let sql = debug_count_sql_with_literals(src, &range);
            let plan = explain_plan(&conn, &sql);
            assert_no_table_scan(&plan, &format!("{desc} range={range:?}"));
        }
    }
}
