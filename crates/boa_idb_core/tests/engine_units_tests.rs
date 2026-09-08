//! Unit tests for the core engine state machines (P1-3 coverage drive).
//!
//! `engine::{connection, transaction, registry, request, cursor}` are small
//! synchronous units with no backend behind them; these tests pin their
//! documented transitions (initial states, guards, saturation, error
//! mapping) so the TZ §13.2 line-coverage gate measures exercised code
//! rather than dead weight. `CoreCursor` runs against a scripted fake
//! [`BackendCursor`] plus real key/SCF codecs.

use boa_idb_core::backend::capabilities::BackendCapabilities;
use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendCursor;
use boa_idb_core::backend::types::{CursorSeek, DatabaseMeta};
use boa_idb_core::clone::encode::encode_scf;
use boa_idb_core::clone::scvalue::ScValue;
use boa_idb_core::engine::connection::{Connection, ConnectionState};
use boa_idb_core::engine::cursor::{CoreCursor, CursorState};
use boa_idb_core::engine::registry::Registry;
use boa_idb_core::engine::request::{Request, RequestState};
use boa_idb_core::engine::transaction::{Transaction, TxnState};
use boa_idb_core::error::{IdbError, KeyError, KeyPathError, ScError};
use boa_idb_core::key::encode::encode_key;
use boa_idb_core::key::utf16::Utf16String;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use boa_idb_core::proto::{Direction, Durability, OpOutcome, Operation, SourceRef, TxnMode};

fn meta(name: &str) -> DatabaseMeta {
    DatabaseMeta {
        name: Utf16String::from(name),
        version: 1,
        stores: Vec::new(),
        next_store_id: 1,
        next_index_id: 1,
    }
}

#[test]
fn connection_lifecycle() {
    let mut conn = Connection::new(7, "db".to_string());
    assert_eq!(conn.id, 7);
    assert_eq!(conn.database_name, "db");
    assert_eq!(conn.state(), ConnectionState::Opening);
    assert!(!conn.is_open());
    assert!(!conn.is_closed());

    conn.set_open();
    assert_eq!(conn.state(), ConnectionState::Open);
    assert!(conn.is_open());
    assert!(!conn.is_closed());

    conn.start_close();
    assert_eq!(conn.state(), ConnectionState::Closing);
    assert!(!conn.is_open());
    assert!(conn.is_closed());

    conn.finish_close();
    assert_eq!(conn.state(), ConnectionState::Closed);
    assert!(conn.is_closed());
}

#[test]
fn transaction_commit_path() {
    let mut txn = Transaction::new(1, TxnMode::ReadWrite, vec![10, 20], Durability::Strict);
    assert_eq!(txn.id, 1);
    assert_eq!(txn.mode, TxnMode::ReadWrite);
    assert_eq!(txn.durability, Durability::Strict);
    assert_eq!(txn.state(), TxnState::Active);
    assert!(txn.is_active());
    assert!(!txn.is_finished());
    assert!(txn.includes_store(10));
    assert!(!txn.includes_store(99));

    assert!(txn.start_commit());
    assert_eq!(txn.state(), TxnState::Committing);
    // Second commit is rejected: not Active anymore.
    assert!(!txn.start_commit());
    txn.finish_commit();
    assert_eq!(txn.state(), TxnState::Committed);
    assert!(txn.is_finished());
    // Committed transactions cannot abort.
    assert!(!txn.start_abort());
}

#[test]
fn transaction_abort_path() {
    let mut txn = Transaction::new(2, TxnMode::ReadOnly, vec![], Durability::Relaxed);
    assert!(txn.start_abort());
    assert_eq!(txn.state(), TxnState::Aborting);
    assert!(!txn.is_finished());
    txn.finish_abort();
    assert_eq!(txn.state(), TxnState::Aborted);
    assert!(txn.is_finished());
    assert!(!txn.is_active());
    // Aborted transactions cannot abort again.
    assert!(!txn.start_abort());
}

#[test]
fn registry_register_and_connections() {
    let mut reg = Registry::new();
    assert_eq!(reg.connection_count("missing"), 0);
    assert!(reg.get("missing").is_none());

    reg.register(meta("a"));
    reg.register(meta("b"));
    assert_eq!(reg.get("a").unwrap().version, 1);
    assert!(reg.get_mut("a").is_some());
    assert!(reg.get_mut("missing").is_none());

    reg.add_connection("a");
    reg.add_connection("a");
    // Unknown names are ignored, never created.
    reg.add_connection("missing");
    assert_eq!(reg.connection_count("a"), 2);
    reg.remove_connection("a");
    assert_eq!(reg.connection_count("a"), 1);
    // Saturation: never underflows.
    reg.remove_connection("b");
    reg.remove_connection("missing");
    assert_eq!(reg.connection_count("b"), 0);

    let mut names = reg.list_names();
    names.sort();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);

    let removed = reg.unregister("a").unwrap();
    assert_eq!(removed.connection_count, 1);
    assert!(reg.get("a").is_none());
    assert!(reg.unregister("a").is_none());
}

#[test]
fn request_state_transitions() {
    let op = Operation::Put {
        store: 3,
        key: Some(Key::Number(1.0)),
        value: ScValue::Null,
        no_overwrite: false,
    };
    let mut req = Request::new(42, op);
    assert_eq!(req.id, 42);
    assert_eq!(req.state, RequestState::Pending);

    req.state = RequestState::Executing;
    assert_eq!(req.state, RequestState::Executing);

    req.state = RequestState::Done(OpOutcome::Empty);
    assert_eq!(req.state, RequestState::Done(OpOutcome::Empty));

    req.state = RequestState::Failed("boom".to_string());
    assert_eq!(req.state, RequestState::Failed("boom".to_string()));
}

#[test]
fn capabilities_defaults() {
    let caps = BackendCapabilities::default();
    assert!(caps.snapshot_isolation);
    assert!(!caps.durable);
    assert!(caps.concurrent);
    assert_eq!(caps.max_key_size, None);
    assert_eq!(caps.max_value_size, None);
    assert_eq!(caps, BackendCapabilities::default());
}

#[test]
fn error_conversions_wrap_source_text() {
    let from_key: IdbError = KeyError::InvalidValue("bad key".to_string()).into();
    assert!(matches!(from_key, IdbError::Data(ref m) if m.contains("bad key")));

    let from_path: IdbError = KeyPathError::InvalidSyntax("bad path".to_string()).into();
    assert!(matches!(from_path, IdbError::Data(ref m) if m.contains("bad path")));

    let from_scf: IdbError = ScError::InvalidMagic.into();
    assert!(matches!(from_scf, IdbError::DataClone(_)));
}

/// Scripted backend cursor driving `CoreCursor` without storage.
struct FakeCursor {
    keys: Vec<Vec<u8>>,
    pkeys: Vec<Vec<u8>>,
    values: Vec<Option<Vec<u8>>>,
    pos: Option<usize>,
    fail: bool,
}

impl FakeCursor {
    fn live(keys: Vec<Vec<u8>>, value: Vec<u8>) -> Self {
        let n = keys.len();
        Self {
            keys,
            pkeys: vec![vec![9u8]; n],
            values: vec![Some(value); n],
            pos: None,
            fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            keys: Vec::new(),
            pkeys: Vec::new(),
            values: Vec::new(),
            pos: None,
            fail: true,
        }
    }
}

impl BackendCursor for FakeCursor {
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError> {
        if self.fail {
            return Err(BackendError::Internal("boom".to_string()));
        }
        if target == CursorSeek::First {
            self.pos = Some(0);
            Ok(!self.keys.is_empty())
        } else {
            self.pos = None;
            Ok(false)
        }
    }

    fn step(&mut self, count: u32) -> Result<bool, BackendError> {
        if self.fail {
            return Err(BackendError::Internal("boom".to_string()));
        }
        let next = self.pos.unwrap_or(0) + count as usize;
        if next < self.keys.len() {
            self.pos = Some(next);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn current_key(&self) -> &[u8] {
        self.pos.map_or(&[], |i| self.keys[i].as_slice())
    }

    fn current_primary_key(&self) -> &[u8] {
        self.pos.map_or(&[], |i| self.pkeys[i].as_slice())
    }

    fn current_value(&self) -> Option<&[u8]> {
        self.pos.and_then(|i| self.values[i].as_deref())
    }
}

fn encoded_number_key(n: f64) -> Vec<u8> {
    let limits = LimitConfig::default();
    let mut out = Vec::new();
    encode_key(&Key::Number(n), &mut out, &limits).unwrap();
    out
}

fn encoded_number_value(n: f64) -> Vec<u8> {
    encode_scf(&ScValue::Number(n), &LimitConfig::default()).unwrap()
}

fn core_cursor(rows: usize) -> (CoreCursor<'static>, &'static LimitConfig) {
    // Leak the limits: the cursor borrows them, and the test needs no drop.
    let limits: &'static LimitConfig = Box::leak(Box::new(LimitConfig::default()));
    let keys = vec![encoded_number_key(4.0); rows];
    let backend: Box<dyn BackendCursor + 'static> =
        Box::new(FakeCursor::live(keys, encoded_number_value(4.0)));
    let cursor = CoreCursor::new(
        1,
        SourceRef::Store(5),
        Direction::Next,
        false,
        backend,
        limits,
    );
    assert_eq!(cursor.id, 1);
    assert_eq!(cursor.source, SourceRef::Store(5));
    assert_eq!(cursor.direction, Direction::Next);
    assert!(!cursor.key_only);
    (cursor, limits)
}

#[test]
fn core_cursor_walk_decodes_key_and_value() {
    let (mut cursor, _) = core_cursor(2);
    assert_eq!(cursor.state(), CursorState::Active);

    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.state(), CursorState::Active);
    assert_eq!(cursor.current_key().unwrap(), Some(Key::Number(4.0)));
    // Fake primary keys are raw 0x09, which is not a valid key encoding.
    assert!(cursor.current_primary_key().is_err());
    assert_eq!(cursor.current_value().unwrap(), Some(ScValue::Number(4.0)));

    assert!(cursor.advance(1).unwrap());
    assert_eq!(cursor.current_key().unwrap(), Some(Key::Number(4.0)));

    // Stepping past the end exhausts the cursor; reads go None.
    assert!(!cursor.advance(1).unwrap());
    assert_eq!(cursor.state(), CursorState::Exhausted);
    assert!(!cursor.advance(1).unwrap());
    assert_eq!(cursor.current_key().unwrap(), None);
    assert_eq!(cursor.current_value().unwrap(), None);

    cursor.close();
    assert_eq!(cursor.state(), CursorState::Closed);
}

#[test]
fn core_cursor_empty_seek_exhausts() {
    let (mut cursor, _) = core_cursor(0);
    assert!(!cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.state(), CursorState::Exhausted);
    // Unknown seek targets miss.
    assert!(!cursor.seek(CursorSeek::Key(vec![1, 2, 3])).unwrap());
}

#[test]
fn core_cursor_backend_errors_map_to_data() {
    let limits: &'static LimitConfig = Box::leak(Box::new(LimitConfig::default()));
    let backend: Box<dyn BackendCursor + 'static> = Box::new(FakeCursor::failing());
    let mut cursor = CoreCursor::new(
        4,
        SourceRef::Store(5),
        Direction::Next,
        false,
        backend,
        limits,
    );
    let seek_err = cursor.seek(CursorSeek::First).unwrap_err();
    assert!(
        matches!(seek_err, IdbError::Data(ref m) if m.contains("boom")),
        "seek maps backend failure, got {seek_err:?}"
    );
    let step_err = cursor.advance(1).unwrap_err();
    assert!(
        matches!(step_err, IdbError::Data(ref m) if m.contains("boom")),
        "step maps backend failure, got {step_err:?}"
    );
}

#[test]
fn core_cursor_key_only_hides_value() {
    let limits: &'static LimitConfig = Box::leak(Box::new(LimitConfig::default()));
    let backend: Box<dyn BackendCursor + 'static> = Box::new(FakeCursor::live(
        vec![encoded_number_key(1.0)],
        encoded_number_value(1.0),
    ));
    let mut cursor = CoreCursor::new(
        2,
        SourceRef::Store(5),
        Direction::Next,
        true,
        backend,
        limits,
    );
    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert_eq!(cursor.current_value().unwrap(), None);
    assert_eq!(cursor.current_key().unwrap(), Some(Key::Number(1.0)));
}

#[test]
fn core_cursor_corrupt_payload_errors() {
    let limits: &'static LimitConfig = Box::leak(Box::new(LimitConfig::default()));
    // 0x00 is not a valid encoded key tag; empty value decodes fail too.
    let backend: Box<dyn BackendCursor + 'static> =
        Box::new(FakeCursor::live(vec![vec![0xFF]], vec![0x00]));
    let mut cursor = CoreCursor::new(
        3,
        SourceRef::Store(5),
        Direction::Next,
        false,
        backend,
        limits,
    );
    assert!(cursor.seek(CursorSeek::First).unwrap());
    assert!(cursor.current_key().is_err());
    assert!(cursor.current_value().is_err());
}
