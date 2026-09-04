//! IndexedDB request driver for the JS layer.
//!
//! The driver executes the asynchronous IndexedDB pipeline the WPT tests rely
//! on: pending `IDBFactory.open` / `deleteDatabase` requests, per-transaction
//! request queues, cursor iteration, transaction lifecycle (auto-commit,
//! explicit commit, abort) and event dispatch.
//!
//! All state lives in [`DriverState`] (Context HostDefined data). It holds only
//! plain Rust values; request/cursor/transaction `JsObject`s are kept alive by
//! the JS side and mirrored in traced registries on [`crate::runtime::IdbRuntime`].

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use boa_engine::class::Class;
use boa_engine::object::builtins::JsArray;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb_core::backend::traits::BackendTxn;
use boa_idb_core::backend::types::{CursorSeek, DatabaseMeta};
use boa_idb_core::clone::decode::decode_scf;
use boa_idb_core::clone::scvalue::ScValue;
use boa_idb_core::engine::keygen::KeyGenerator;
use boa_idb_core::engine::open_queue::{OpenQueue, OpenQueueAction};
use boa_idb_core::engine::scheduler::{TransactionScheduler, TxnQueueItem};
use boa_idb_core::error::IdbError;
use boa_idb_core::key::compare::compare_keys;
use boa_idb_core::key::encode::{decode_key, encode_key};
use boa_idb_core::key::path::KeyPath;
use boa_idb_core::key::range::EncodedRange;
use boa_idb_core::key::value::Key;
use boa_idb_core::limits::LimitConfig;
use boa_idb_core::proto::{Direction, IndexId, SourceRef, StoreId, TxnId, TxnMode};

use crate::api::request::{IdBRequest, ReadyState};
use crate::api::transaction::IdBTransaction;
use crate::convert::key::key_to_value;
use crate::convert::value::deserialize_from_storage;
use crate::engine::IdbEngine;
use crate::runtime::IdbRuntime;

/// Default limit configuration for the driver.
fn limits() -> LimitConfig {
    LimitConfig::default()
}

/// Converts a key-encoding failure to an `IdbError`.
#[allow(clippy::needless_pass_by_value)]
fn encode_key_err(e: boa_idb_core::error::KeyError) -> IdbError {
    IdbError::Data(format!("Key encoding failed: {e}"))
}

/// Encodes a key to bytes.
fn enc_key(key: &Key, lim: &LimitConfig) -> Result<Vec<u8>, IdbError> {
    let mut out = Vec::new();
    encode_key(key, &mut out, lim).map_err(encode_key_err)?;
    Ok(out)
}

/// Converts a backend error to an `IdbError`.
///
/// `Constraint` violations are preserved so callers observe
/// `ConstraintError` rather than a generic `DataError`.
fn backend_err(e: boa_idb_core::backend::error::BackendError) -> IdbError {
    match e {
        boa_idb_core::backend::error::BackendError::Constraint(msg) => IdbError::Constraint(msg),
        other => IdbError::Data(format!("Backend error: {other}")),
    }
}

/// Store id for a source.
fn store_of(source: SourceRef) -> StoreId {
    match source {
        SourceRef::Store(id) => id,
        SourceRef::Index { store, .. } => store,
    }
}

/// A key range in plain form (converted from an `IDBKeyRange` object).
#[derive(Debug, Clone, PartialEq)]
pub enum RangeData {
    /// Match all records.
    All,
    /// Match exactly one key.
    Only(Key),
    /// Bounded range.
    Bounds {
        /// Lower bound `(key, open)`.
        lower: Option<(Key, bool)>,
        /// Upper bound `(key, open)`.
        upper: Option<(Key, bool)>,
    },
}

impl RangeData {
    /// Encodes the range to backend form.
    pub fn to_encoded(&self, lim: &LimitConfig) -> Result<EncodedRange, IdbError> {
        Ok(match self {
            Self::All => EncodedRange::all(),
            Self::Only(k) => EncodedRange::only(enc_key(k, lim)?),
            Self::Bounds {
                lower: None,
                upper: None,
            } => EncodedRange::all(),
            Self::Bounds {
                lower: Some((l, lo)),
                upper: None,
            } => EncodedRange::lower_bound(enc_key(l, lim)?, *lo),
            Self::Bounds {
                lower: None,
                upper: Some((u, uo)),
            } => EncodedRange::upper_bound(enc_key(u, lim)?, *uo),
            Self::Bounds {
                lower: Some((l, lo)),
                upper: Some((u, uo)),
            } => EncodedRange::bound(enc_key(l, lim)?, *lo, enc_key(u, lim)?, *uo),
        })
    }
}

/// One queued operation against a logical IndexedDB transaction.
#[derive(Debug, Clone)]
pub enum PendingOp {
    /// Read the first matching record.
    Get { source: SourceRef, range: RangeData },
    /// Count matching records.
    Count { source: SourceRef, range: RangeData },
    /// getKey(query).
    GetKey { source: SourceRef, range: RangeData },
    /// getAll / getAllKeys.
    GetAll {
        source: SourceRef,
        range: RangeData,
        limit: Option<u32>,
        keys_only: bool,
    },
    /// getAllRecords (IDBRecord snapshots).
    GetAllRecords {
        source: SourceRef,
        range: RangeData,
        limit: Option<u32>,
        direction: Direction,
    },
    /// Put/add a record.
    Put {
        store_id: StoreId,
        value: ScValue,
        explicit_key: Option<Key>,
        no_overwrite: bool,
    },
    /// Delete matching records.
    Delete { store_id: StoreId, range: RangeData },
    /// Clear a store.
    Clear { store_id: StoreId },
    /// Open a cursor.
    OpenCursor {
        source: SourceRef,
        range: RangeData,
        direction: Direction,
        key_only: bool,
    },
    /// Operate on an already-open cursor.
    CursorOp {
        cursor_id: u64,
        action: CursorAction,
    },
}

/// Cursor action.
#[derive(Debug, Clone)]
pub enum CursorAction {
    /// `advance(count)`.
    Advance(u32),
    /// `continue(key?)`: `None` = no key argument.
    Continue(Option<Key>),
    /// `continuePrimaryKey(key, primaryKey)`.
    ContinuePrimaryKey { key: Key, primary_key: Key },
    /// `update(value)`.
    Update(ScValue),
    /// `delete()`.
    Delete,
}

/// Kind of a pending database open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenKind {
    /// `IDBFactory.open`.
    Open,
    /// `IDBFactory.deleteDatabase`.
    Delete,
}

/// A pending database open / delete.
#[derive(Debug, Clone)]
pub struct PendingOpen {
    /// Kind.
    pub kind: OpenKind,
    /// Database name.
    pub name: String,
    /// Requested version (0 = delete, or resolved for opens).
    pub version: u64,
    /// Connection id for the resulting `IDBDatabase`.
    pub conn_id: u64,
    /// Request id.
    pub request_id: u64,
    /// True while an upgrade transaction is draining.
    pub upgrade_active: bool,
    /// The versionchange transaction id during an upgrade.
    pub upgrade_txn_id: Option<u64>,
    /// Registered once in the core open queue.
    pub queued: bool,
    /// Whether the `blocked` event was already fired for this open.
    pub blocked_fired: bool,
    /// Upgrade version pair once `StartUpgrade` fired.
    pub upgrade_versions: Option<(u64, u64)>,
}

/// A driver-owned transaction handle.
///
/// The backend transaction is `'static` (see `Database::begin`), so the
/// handle fully owns it — no self-referential borrowing. Schema snapshots
/// (`meta`) are tracked per handle: upgrade transactions mutate their copy
/// as schema ops execute, and the committed backend meta is authoritative
/// afterwards.
pub struct TxnHandle {
    /// Transaction id.
    pub txn_id: TxnId,
    /// Connection id.
    pub conn_id: u64,
    /// Database name.
    pub db_name: String,
    /// Mode.
    pub mode: TxnMode,
    /// Store scope (ids).
    pub scope: Vec<StoreId>,
    /// Durability hint.
    pub durability: boa_idb_core::proto::Durability,
    /// Schema snapshot (mutated by schema ops on upgrade transactions).
    pub meta: DatabaseMeta,
    /// Backend transaction (`None` until the scheduler starts it).
    pub backend: Option<Box<dyn BackendTxn>>,
    /// Finished (committed or aborted).
    pub finished: bool,
    /// Transaction is currently active (accepts new requests).
    pub active: bool,
    /// Task epoch of the last activity (creation or own-request dispatch).
    ///
    /// [`DriverState::task_epoch`] advances on every macro-task boundary
    /// (timer fire). `commit()` throws `InvalidStateError` when the epochs
    /// differ: the transaction went inactive across tasks (§2.7).
    pub active_epoch: u64,
    /// Explicit `commit()` was called.
    pub explicit_commit: bool,
    /// Stores whose physical deletion is deferred until upgrade commit.
    pub deleted_store_ids: Vec<StoreId>,
    /// Key generators per store.
    pub keygens: HashMap<StoreId, KeyGenerator>,
}

/// A live cursor state (kept alive by the JS `IDBCursor`).
pub struct CursorState {
    /// Cursor id.
    pub cursor_id: u64,
    /// Owning transaction.
    pub txn_id: TxnId,
    /// Request that opened the cursor (reused for iteration).
    pub request_id: u64,
    /// Source.
    pub source: SourceRef,
    /// Direction.
    pub direction: Direction,
    /// Key-only.
    pub key_only: bool,
    /// Materialized rows in iteration order.
    pub rows: Vec<CursorRow>,
    /// Current index (`rows.len()` = exhausted).
    pub pos: usize,
    /// Whether a cursor request is currently pending (cursor is being iterated).
    pub pending: bool,
}

/// A materialized cursor row.
#[derive(Debug, Clone)]
pub struct CursorRow {
    /// Key.
    pub key: Key,
    /// Primary key.
    pub primary_key: Key,
    /// Value (`None` for key-only cursors).
    pub value: Option<ScValue>,
}

/// Snapshot of a cursor for the API layer (owned, lock-free use).
#[derive(Debug, Clone)]
pub struct CursorView {
    /// Cursor id.
    pub cursor_id: u64,
    /// Owning transaction.
    pub txn_id: TxnId,
    /// Request that opened the cursor.
    pub request_id: u64,
    /// Direction.
    pub direction: Direction,
    /// Key-only cursor.
    pub key_only: bool,
    /// Whether the source is an index.
    pub is_index: bool,
    /// Key path of the source object store.
    pub key_path: KeyPath,
    /// Current row (None = exhausted).
    pub current: Option<CursorRowView>,
    /// Whether a navigation request is pending.
    pub pending: bool,
}

/// Snapshot of a cursor row for the API layer.
#[derive(Debug, Clone)]
pub struct CursorRowView {
    /// Key.
    pub key: Key,
    /// Primary key.
    pub primary_key: Key,
    /// Value (`None` for key-only cursors).
    pub value: Option<ScValue>,
}

/// Returns an owned snapshot of a cursor, if live.
pub fn cursor_view(d: &DriverState, cursor_id: u64) -> Option<CursorView> {
    d.cursors.get(&cursor_id).map(|c| CursorView {
        cursor_id,
        txn_id: c.txn_id,
        request_id: c.request_id,
        direction: c.direction,
        key_only: c.key_only,
        is_index: matches!(c.source, SourceRef::Index { .. }),
        key_path: d
            .txns
            .get(&c.txn_id)
            .and_then(|txn| {
                txn.meta
                    .stores
                    .iter()
                    .find(|store| store.id == store_of(c.source))
            })
            .map_or(KeyPath::Empty, |store| store.key_path.clone()),
        current: c.current().map(|row| CursorRowView {
            key: row.key.clone(),
            primary_key: row.primary_key.clone(),
            value: row.value.clone(),
        }),
        pending: c.pending,
    })
}

/// Marks a live cursor as waiting for its next request completion.
pub fn set_cursor_pending(context: &mut Context, cursor_id: u64, pending: bool) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        let mut d = crate::runtime::lock_mutex(&runtime.driver);
        if let Some(cursor) = d.cursors.get_mut(&cursor_id) {
            cursor.pending = pending;
        }
    }
}

impl CursorState {
    /// The current row, if the position is valid.
    pub fn current(&self) -> Option<&CursorRow> {
        (self.pos < self.rows.len()).then(|| &self.rows[self.pos])
    }
}

/// Applies a cursor action to a cursor; returns `false` when exhausted.
fn apply_cursor_action(state: &mut CursorState, action: &CursorAction) -> bool {
    let is_prev = state.direction.is_prev();
    match action {
        CursorAction::Advance(n) => {
            // `rows` is materialized in cursor direction, so advancing is
            // always movement toward the next vector element.
            state.pos = state.pos.saturating_add(*n as usize);
        }
        CursorAction::Continue(None) => {
            state.pos = state.pos.saturating_add(1);
        }
        CursorAction::Continue(Some(target)) => {
            if is_prev {
                cursor_seek_key_prev(state, target);
            } else {
                cursor_seek_key_next(state, target);
            }
        }
        CursorAction::ContinuePrimaryKey { key, primary_key } => {
            cursor_seek_key_primary(state, key, primary_key, is_prev);
        }
        CursorAction::Update(_) | CursorAction::Delete => {}
    }
    let _ = is_prev;
    state.pos < state.rows.len()
}

/// For a `next` direction: first row strictly after the current whose key is
/// not less than `target`.
fn cursor_seek_key_next(state: &mut CursorState, target: &Key) {
    for i in (state.pos + 1)..state.rows.len() {
        if compare_keys(&state.rows[i].key, target) != Ordering::Less {
            state.pos = i;
            return;
        }
    }
    state.pos = state.rows.len();
}

/// For a `prev` direction: greatest row strictly before/at the current whose
/// key is not greater than `target`.
fn cursor_seek_key_prev(state: &mut CursorState, target: &Key) {
    for i in (state.pos + 1)..state.rows.len() {
        if compare_keys(&state.rows[i].key, target) != Ordering::Greater {
            state.pos = i;
            return;
        }
    }
    state.pos = state.rows.len();
}

/// For `continuePrimaryKey`: first row after the current position that is
/// not less than the target in the direction's tuple order.
///
/// Directions are restricted to `next`/`prev` (never `*unique`) by the API
/// layer, so tuple order is plain lexicographic `(key, primary_key)`:
/// ascending for `next`, descending for `prev`.
fn cursor_seek_key_primary(state: &mut CursorState, key: &Key, primary: &Key, is_prev: bool) {
    for i in (state.pos + 1)..state.rows.len() {
        let k = compare_keys(&state.rows[i].key, key);
        let pk = compare_keys(&state.rows[i].primary_key, primary);
        let fits = if is_prev {
            k == Ordering::Less || (k == Ordering::Equal && pk != Ordering::Greater)
        } else {
            k == Ordering::Greater || (k == Ordering::Equal && pk != Ordering::Less)
        };
        if fits {
            state.pos = i;
            return;
        }
    }
    state.pos = state.rows.len();
}

/// All driver state.
#[derive(Default)]
pub struct DriverState {
    /// Pending opens.
    pub opens: VecDeque<PendingOpen>,
    /// Core open queues per database (version compare, blocked, upgrade).
    pub open_queues: HashMap<String, OpenQueue>,
    /// Open transaction handles.
    pub txns: HashMap<TxnId, TxnHandle>,
    /// Scheduler gating transaction starts (FIFO fairness, §2.7.2).
    pub scheduler: TransactionScheduler,
    /// Request id → owning transaction.
    pub txn_of_request: HashMap<u64, TxnId>,
    /// Queued op per request.
    pub ops: HashMap<u64, PendingOp>,
    /// Per-transaction FIFO queues.
    pub txn_queues: HashMap<TxnId, VecDeque<u64>>,
    /// Live cursors.
    pub cursors: HashMap<u64, CursorState>,
    /// Connections: id → (database name, closed).
    pub connections: HashMap<u64, (String, bool)>,
    /// A pump job is already scheduled.
    pub pump_scheduled: bool,
    /// Id counters.
    pub next_request_id: u64,
    pub next_txn_id: u64,
    pub next_cursor_id: u64,
    pub next_conn_id: u64,
    /// Macro-task epoch: bumped on every timer fire (task boundary).
    ///
    /// Transactions record the epoch of their last activity
    /// ([`TxnHandle::active_epoch`]); a mismatch means the scope went
    /// inactive across tasks.
    pub task_epoch: u64,
}

impl DriverState {
    /// Allocates a request id.
    pub fn alloc_request(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    /// Allocates a transaction id.
    pub fn alloc_txn(&mut self) -> u64 {
        let id = self.next_txn_id;
        self.next_txn_id += 1;
        id
    }

    /// Allocates a cursor id.
    pub fn alloc_cursor(&mut self) -> u64 {
        let id = self.next_cursor_id;
        self.next_cursor_id += 1;
        id
    }

    /// Allocates a connection id.
    pub fn alloc_conn(&mut self) -> u64 {
        let id = self.next_conn_id;
        self.next_conn_id += 1;
        id
    }

    /// Enqueues a request on a transaction.
    pub fn enqueue(&mut self, txn_id: u64, request_id: u64, op: PendingOp) {
        self.txn_of_request.insert(request_id, txn_id);
        self.ops.insert(request_id, op);
        self.txn_queues
            .entry(txn_id)
            .or_default()
            .push_back(request_id);
    }
}

/// Result of request execution.
enum RawOutcome {
    /// No value.
    Empty,
    /// A key.
    Key(Key),
    /// An optional value.
    Value(Option<ScValue>),
    /// Multiple keys.
    Keys(Vec<Key>),
    /// Multiple values.
    Values(Vec<ScValue>),
    /// Record snapshots (getAllRecords).
    Records(Vec<CursorRow>),
    /// A count.
    Count(u64),
}

/// Outcome of executing one op.
enum OpExec {
    /// Plain outcome.
    Outcome(RawOutcome),
    /// A cursor was opened.
    CursorOpened { cursor_id: u64 },
    /// A cursor was iterated.
    CursorContinued { cursor_id: u64, reached_end: bool },
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Public entry point for pumps: drives one full turn.
///
/// Lock discipline: engine/driver guards are only ever held across pure
/// state steps. Every scope ends before event dispatch, so reentrant API
/// calls from JS handlers lock freely and cannot deadlock.
pub fn drive_turn(context: &mut Context) -> JsResult<bool> {
    // Clone the Arc handles (not the data) so the borrows of `context` end
    // before event dispatch re-borrows it mutably.
    let engine_handle = context
        .get_data::<IdbRuntime>()
        .map(|r| Arc::clone(&r.engine));
    let driver_handle = context
        .get_data::<IdbRuntime>()
        .map(|r| Arc::clone(&r.driver));
    let (Some(engine_handle), Some(driver_handle)) = (engine_handle, driver_handle) else {
        return Ok(false);
    };
    // Lazily initialize the engine (scoped lock, no JS).
    {
        let (factory, key) = {
            let runtime = context
                .get_data::<IdbRuntime>()
                .ok_or_else(|| JsNativeError::error().with_message("IndexedDB not initialized"))?;
            (runtime.backend_factory.clone(), runtime.storage_key.clone())
        };
        let mut engine_opt = crate::runtime::lock_mutex(&engine_handle);
        if engine_opt.is_none() {
            let engine = IdbEngine::new(factory, &key)
                .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;
            *engine_opt = Some(engine);
        }
    }
    driver_turn(&engine_handle, &driver_handle, context)
    // NOTE: progress (not pending-presence) drives the pump loop, so a
    // stalled-but-queued open (blocked) does not spin forever.
}

/// Reports whether the driver still has work, given its handle.
fn has_pending_work_tuple(driver: &Arc<std::sync::Mutex<DriverState>>) -> bool {
    let d = crate::runtime::lock_mutex(driver);
    !d.opens.is_empty() || d.txn_queues.values().any(|q| !q.is_empty())
}

/// Returns `true` when the context still has pending IDB work.
pub fn has_pending_work(context: &Context) -> bool {
    context.get_data::<IdbRuntime>().is_some_and(|r| {
        let d = crate::runtime::lock_mutex(&r.driver);
        !d.opens.is_empty() || d.txn_queues.values().any(|q| !q.is_empty())
    })
}

/// Allocates a request id for a new request.
pub fn alloc_request_id(context: &Context) -> u64 {
    context
        .get_data::<IdbRuntime>()
        .map(|r| crate::runtime::lock_mutex(&r.driver).alloc_request())
        .unwrap_or_default()
}

/// Allocates a transaction id.
pub fn alloc_txn_id(context: &Context) -> u64 {
    context
        .get_data::<IdbRuntime>()
        .map(|r| crate::runtime::lock_mutex(&r.driver).alloc_txn())
        .unwrap_or_default()
}

/// Allocates a connection id.
pub fn alloc_conn_id(context: &Context) -> u64 {
    context
        .get_data::<IdbRuntime>()
        .map(|r| crate::runtime::lock_mutex(&r.driver).alloc_conn())
        .unwrap_or_default()
}

/// Enqueues an operation on a transaction. Returns nothing (op already has the
/// request id).
pub fn enqueue_op(context: &Context, txn_id: u64, request_id: u64, op: PendingOp) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        crate::runtime::lock_mutex(&runtime.driver).enqueue(txn_id, request_id, op);
    }
}

/// Enqueues a database open / delete.
pub fn enqueue_open(context: &Context, open: PendingOpen) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        crate::runtime::lock_mutex(&runtime.driver)
            .opens
            .push_back(open);
    }
}

/// Marks a connection closed and advances its database open queue.
pub fn on_connection_closed(context: &mut Context, conn_id: u64) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        let mut d = crate::runtime::lock_mutex(&runtime.driver);
        if let Some(entry) = d.connections.get_mut(&conn_id) {
            entry.1 = true;
        }
    }
    crate::runtime::schedule_pump(context);
}

/// Driver turn implementation.
///
/// Returns `true` when any state changed. The pump loops while turns make
/// progress and stops otherwise (e.g. an open blocked on live connections
/// waits for a future `close()`, which schedules a new pump).
fn driver_turn(
    engine_handle: &Arc<std::sync::Mutex<Option<IdbEngine>>>,
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
) -> JsResult<bool> {
    let mut progressed = false;

    // Phase 1: opens / deletes (version compare, blocked, upgrades).
    progressed |= process_opens(engine_handle, driver_handle, context)?;

    // Phase 2: start scheduler-ready transactions (backend handles).
    progressed |= start_ready_txns(engine_handle, driver_handle, context)?;

    // Phase 3: transaction request queues (one op per started txn).
    progressed |= process_txn_requests(driver_handle, context)?;

    // Phase 3.25: microtask checkpoint (manual-pump embeddings only).
    //
    // Request completions resolve promises whose continuations may attach
    // transaction watchers (`promiseForTransaction`) or enqueue follow-up
    // requests. They must run BEFORE Phase 4 finishes transactions,
    // otherwise the `complete` event fires before the watcher exists (lost
    // wakeup) and the waiter hangs forever. Auto-pump embeddings skip
    // this: completions there flow through scheduled jobs, and reentering
    // the job queue from inside a pump job is not allowed.
    if context
        .get_data::<IdbRuntime>()
        .is_none_or(|r| !r.auto_pump.get())
    {
        context.run_jobs()?;
    }

    // Phase 3.5: scope-close idle transactions (task-boundary approximation).
    deactivate_idle_txns(driver_handle, context);

    // Phase 4: finish drained non-versionchange transactions.
    progressed |= finish_txns(driver_handle, context)?;

    Ok(progressed)
}

/// Records a macro-task boundary (timer fire): scopes go inactive.
///
/// Embeddings with virtual time (the WPT runner) call this whenever a timer
/// callback fires. A later `commit()` on a transaction whose epoch was not
/// refreshed by one of its own request dispatches since then throws
/// `InvalidStateError` (§2.7: the scope is inactive in the new task).
pub fn note_timer_task(context: &mut Context) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        crate::runtime::lock_mutex(&runtime.driver).task_epoch += 1;
    }
}

/// Refreshes a transaction's activity epoch to the current task epoch.
///
/// Called when one of the transaction's own request events is dispatched:
/// the scope is active in the dispatch task, so a `commit()` from its
/// handlers (or the same task) is legal.
fn refresh_txn_epoch(driver_handle: &Arc<std::sync::Mutex<DriverState>>, txn_id: TxnId) {
    let mut d = crate::runtime::lock_mutex(driver_handle);
    let epoch = d.task_epoch;
    if let Some(handle) = d.txns.get_mut(&txn_id) {
        handle.active_epoch = epoch;
    }
}

// ---------------------------------------------------------------------------
// Phases
// ---------------------------------------------------------------------------

/// Marks transactions with drained queues inactive (driver handle + JS object).
///
/// The pump coalesces many JS tasks into one drain, so there is no natural
/// task boundary at which scopes become inactive — yet `finish_txns` only
/// commits inactive (or explicitly committed) transactions. A transaction
/// whose request queue just drained is idle: no handler is running that
/// could still enqueue on it (handlers enqueue synchronously during
/// dispatch), so it is safe to close its scope now. Transactions with queued
/// work (e.g. `keep_alive` spins) stay active, as do finished and
/// versionchange transactions (owned by the open flow).
///
/// Pure bookkeeping + attribute writes; never dispatches.
fn deactivate_idle_txns(driver_handle: &Arc<std::sync::Mutex<DriverState>>, context: &mut Context) {
    let ids: Vec<TxnId> = {
        let mut d = crate::runtime::lock_mutex(driver_handle);
        let mut ids = Vec::new();
        let candidates: Vec<TxnId> = d
            .txns
            .iter()
            .filter(|(_, handle)| {
                !handle.finished && handle.active && handle.mode != TxnMode::VersionChange
            })
            .map(|(id, _)| *id)
            .collect();
        for id in candidates {
            let idle = d.txn_queues.get(&id).is_none_or(VecDeque::is_empty);
            if idle {
                if let Some(handle) = d.txns.get_mut(&id) {
                    handle.active = false;
                }
                ids.push(id);
            }
        }
        ids
    };
    for id in ids {
        if let Some(obj) = crate::runtime::txn_object(context, id) {
            if let Some(mut data) = obj.downcast_mut::<IdBTransaction>() {
                data.active = false;
            }
        }
    }
}

/// Counts live (non-closed) connections of a database.
fn live_connection_count(d: &DriverState, db_name: &str) -> u32 {
    d.connections
        .values()
        .filter(|(name, closed)| name == db_name && !closed)
        .count() as u32
}

/// One actionable open-queue step, fully computed under locks.
enum OpenStep {
    /// Fire `blocked` (once) for an open.
    FireBlocked { open: PendingOpen },
    /// Complete an open with the given committed version.
    OpenConnection { open: PendingOpen, version: u64 },
    /// Fail an open.
    FailOpen { open: PendingOpen, error: IdbError },
    /// Start an upgrade transaction.
    StartUpgrade {
        open: PendingOpen,
        old_version: u64,
        new_version: u64,
    },
    /// Run a delete.
    StartDelete { open: PendingOpen },
    /// Finish a drained upgrade: commit its backend transaction.
    FinishUpgrade { open: PendingOpen, txn_id: TxnId },
}

/// Computes the next open step purely (locks held, no JS, no backend IO).
/// Returns `None` when every open is stalled (blocked / draining).
#[allow(clippy::too_many_lines)]
fn next_open_step(engine: &mut IdbEngine, d: &mut DriverState) -> Option<OpenStep> {
    // 1. Drained upgrades finish first.
    if let Some(open) = d.opens.iter().find(|o| o.upgrade_active).cloned() {
        let txn_id = open.upgrade_txn_id?;
        let drained = d.txn_queues.get(&txn_id).is_none_or(VecDeque::is_empty);
        if drained {
            return Some(OpenStep::FinishUpgrade { open, txn_id });
        }
        // A draining upgrade still counts as pending, not as stall: fall
        // through so other opens are considered too? No — upgrades are
        // exclusive; other opens belonging to the same database wait behind
        // the core queue anyway. Continue scanning other opens.
    }

    let ids: Vec<u64> = d
        .opens
        .iter()
        .filter(|o| !o.upgrade_active)
        .map(|o| o.request_id)
        .collect();
    for request_id in ids {
        let idx = d.opens.iter().position(|o| o.request_id == request_id)?;
        let (kind, name) = {
            let open = &d.opens[idx];
            (open.kind, open.name.clone())
        };

        // Live-connection count first: `queue` below borrows `d` mutably.
        let live = live_connection_count(d, &name);
        let queue = d.open_queues.entry(name.clone()).or_insert_with(|| {
            let current = engine.db_version(&name).unwrap_or(0);
            OpenQueue::new(name.clone(), current)
        });
        queue.set_blocking_connections(live);

        if !d.opens[idx].queued {
            match kind {
                OpenKind::Open => {
                    let version = d.opens[idx].version;
                    queue.enqueue_open(request_id, version);
                }
                OpenKind::Delete => queue.enqueue_delete(request_id),
            }
            d.opens[idx].queued = true;
        }

        if queue.state() == boa_idb_core::engine::open_queue::OpenQueueState::WaitingForConnections
            && live == 0
        {
            match queue.on_all_connections_closed() {
                Some(OpenQueueAction::StartUpgrade {
                    old_version,
                    new_version,
                    ..
                }) => {
                    return Some(OpenStep::StartUpgrade {
                        open: d.opens[idx].clone(),
                        old_version,
                        new_version,
                    });
                }
                Some(OpenQueueAction::StartDelete { .. }) => {
                    return Some(OpenStep::StartDelete {
                        open: d.opens[idx].clone(),
                    });
                }
                Some(_) | None => {}
            }
        }

        if queue.state() != boa_idb_core::engine::open_queue::OpenQueueState::Idle {
            continue;
        }
        match queue.process_next() {
            None => {}
            Some(OpenQueueAction::OpenConnection {
                request_id: rid,
                version,
            }) => {
                debug_assert_eq!(rid, request_id);
                return Some(OpenStep::OpenConnection {
                    open: d.opens[idx].clone(),
                    version,
                });
            }
            Some(OpenQueueAction::FailOpen {
                request_id: rid,
                error,
            }) => {
                debug_assert_eq!(rid, request_id);
                return Some(OpenStep::FailOpen {
                    open: d.opens[idx].clone(),
                    error,
                });
            }
            Some(OpenQueueAction::SendBlocked { .. }) => {
                if !d.opens[idx].blocked_fired {
                    d.opens[idx].blocked_fired = true;
                    return Some(OpenStep::FireBlocked {
                        open: d.opens[idx].clone(),
                    });
                }
            }
            Some(OpenQueueAction::StartUpgrade {
                request_id: rid,
                old_version,
                new_version,
            }) => {
                debug_assert_eq!(rid, request_id);
                return Some(OpenStep::StartUpgrade {
                    open: d.opens[idx].clone(),
                    old_version,
                    new_version,
                });
            }
            Some(OpenQueueAction::StartDelete { .. }) => {
                return Some(OpenStep::StartDelete {
                    open: d.opens[idx].clone(),
                });
            }
        }
    }
    None
}

/// Processes database opens/deletes through the core open queue
/// (version compare, `blocked`, upgrades, deletes). Returns progress.
#[allow(clippy::too_many_lines)]
fn process_opens(
    engine_handle: &Arc<std::sync::Mutex<Option<IdbEngine>>>,
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
) -> JsResult<bool> {
    let mut progressed = false;
    loop {
        // Pure step computation (locks held, no JS, backend reads only).
        let step = {
            let mut engine_guard = crate::runtime::lock_mutex(engine_handle);
            let Some(engine) = engine_guard.as_mut() else {
                return Ok(progressed);
            };
            let mut d = crate::runtime::lock_mutex(driver_handle);
            next_open_step(engine, &mut d)
        };
        let Some(step) = step else { break };
        // Perform the step. Backend/storage work runs lock-free on owned
        // handles; dispatch runs with no guards held anywhere.
        match step {
            OpenStep::FireBlocked { open } => {
                fire_blocked(context, &open)?;
                progressed = true;
            }
            OpenStep::OpenConnection { open, version } => {
                {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    d.connections
                        .insert(open.conn_id, (open.name.clone(), false));
                    d.opens.retain(|o| o.request_id != open.request_id);
                }
                set_open_db_result(context, &open, version)?;
                complete_open_success(context, &open)?;
                progressed = true;
            }
            OpenStep::FailOpen { open, error } => {
                {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    d.opens.retain(|o| o.request_id != open.request_id);
                }
                fail_open(context, &open, &error)?;
                progressed = true;
            }
            OpenStep::StartUpgrade {
                open,
                old_version,
                new_version,
            } => {
                // Backend work under scoped locks (pure); dispatch afterwards.
                // The engine is always initialized by `drive_turn`.
                enum StartOutcome {
                    Started,
                    Failed(IdbError),
                }
                let outcome = {
                    let mut engine_guard = crate::runtime::lock_mutex(engine_handle);
                    let Some(engine) = engine_guard.as_mut() else {
                        drop(engine_guard);
                        break;
                    };
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    match start_upgrade_txn(
                        engine,
                        &mut d,
                        context,
                        &open,
                        old_version,
                        new_version,
                    ) {
                        Ok(txn_id) => {
                            if let Some(slot) =
                                d.opens.iter_mut().find(|o| o.request_id == open.request_id)
                            {
                                slot.upgrade_active = true;
                                slot.upgrade_txn_id = Some(txn_id);
                                slot.upgrade_versions = Some((old_version, new_version));
                            }
                            StartOutcome::Started
                        }
                        Err(e) => {
                            if let Some(queue) = d.open_queues.get_mut(&open.name) {
                                queue.on_operation_failed();
                            }
                            d.opens.retain(|o| o.request_id != open.request_id);
                            StartOutcome::Failed(IdbError::Unknown(e))
                        }
                    }
                };
                match outcome {
                    StartOutcome::Started => {
                        // Re-read the open (versions recorded) for the event.
                        let open = {
                            let d = crate::runtime::lock_mutex(driver_handle);
                            d.opens
                                .iter()
                                .find(|o| o.request_id == open.request_id)
                                .cloned()
                                .unwrap_or(open)
                        };
                        // A throwing handler aborts the whole upgrade.
                        if fire_upgradeneeded(context, &open).is_err() {
                            if let Some(txn_id) = open.upgrade_txn_id {
                                abort_failed_upgrade(driver_handle, context, &open, txn_id);
                            }
                        }
                        progressed = true;
                    }
                    StartOutcome::Failed(e) => {
                        fail_open(context, &open, &e)?;
                    }
                }
            }
            OpenStep::StartDelete { open } => {
                let old = {
                    let mut engine_guard = crate::runtime::lock_mutex(engine_handle);
                    let Some(engine) = engine_guard.as_mut() else {
                        return Ok(progressed);
                    };
                    engine.db_version(&open.name).unwrap_or(0)
                };
                let deleted = {
                    let mut engine_guard = crate::runtime::lock_mutex(engine_handle);
                    let Some(engine) = engine_guard.as_mut() else {
                        return Ok(progressed);
                    };
                    engine.delete_database(&open.name)
                };
                {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    if deleted.is_ok() {
                        if let Some(queue) = d.open_queues.get_mut(&open.name) {
                            queue.on_delete_complete();
                        }
                    } else if let Some(queue) = d.open_queues.get_mut(&open.name) {
                        queue.on_operation_failed();
                    }
                    d.opens.retain(|o| o.request_id != open.request_id);
                }
                match deleted {
                    Ok(()) => complete_delete_success(context, &open, old)?,
                    Err(e) => fail_open(context, &open, &e)?,
                }
                progressed = true;
            }
            OpenStep::FinishUpgrade { open, txn_id } => {
                let connection_closed = crate::runtime::request_object(context, open.request_id)
                    .and_then(|request| {
                        crate::api::request::with_request_ref(&request, |req| req.result.clone())
                            .flatten()
                    })
                    .and_then(|value| value.as_object())
                    .is_some_and(|object| {
                        object
                            .downcast_ref::<crate::api::database::IdBDatabase>()
                            .is_some_and(|data| data.closed)
                    });
                // Take the handle out, commit lock-free, then clean up.
                let (handle, db_name) = {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    (d.txns.remove(&txn_id), open.name.clone())
                };
                if let Some(mut handle) = handle {
                    if let Some(mut backend) = handle.backend.take() {
                        for store_id in &handle.deleted_store_ids {
                            let _ = backend.delete_store(*store_id);
                        }
                        let _ = backend.commit();
                    }
                }
                {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    d.scheduler.forget(txn_id);
                    d.txn_queues.remove(&txn_id);
                    d.txn_of_request.retain(|_, t| *t != txn_id);
                    d.opens.retain(|o| o.request_id != open.request_id);
                    // Release the core open queue: version is committed.
                    if let Some(queue) = d.open_queues.get_mut(&db_name) {
                        if let Some((_, new_version)) = open.upgrade_versions {
                            queue.on_upgrade_complete(new_version);
                        } else {
                            queue.on_operation_failed();
                        }
                    }
                }
                if let Some(txn_obj) = crate::runtime::txn_object(context, txn_id) {
                    if let Some(mut data) = txn_obj.downcast_mut::<IdBTransaction>() {
                        data.active = false;
                        data.finished = true;
                    }
                    let event = make_event(context, "complete", Some(txn_obj.clone()));
                    let _ = dispatch_target(context, &txn_obj, &event);
                }
                crate::runtime::unregister_txn(context, txn_id);
                clear_upgrade_txn(context, &open);
                if connection_closed {
                    clear_open_request_result(context, &open);
                    fail_open(context, &open, &IdbError::Abort)?;
                } else {
                    complete_open_success(context, &open)?;
                }
                progressed = true;
            }
        }
    }
    Ok(progressed)
}

/// Begins the versionchange backend transaction and attaches the upgrade
/// transaction object to the open request.
fn start_upgrade_txn(
    engine: &mut IdbEngine,
    d: &mut DriverState,
    context: &mut Context,
    open: &PendingOpen,
    old_version: u64,
    new_version: u64,
) -> Result<u64, String> {
    let txn_id = d.alloc_txn();
    let mut db_handle = engine
        .open_db_handle(&open.name)
        .map_err(|e| format!("open database: {e}"))?;
    let mut backend = db_handle
        .begin(
            TxnMode::VersionChange,
            &[],
            boa_idb_core::proto::Durability::Default,
        )
        .map_err(|e| format!("begin versionchange: {e}"))?;
    // The target version commits atomically with the schema work (an abort
    // rolls it back together with everything else).
    backend
        .set_version(new_version)
        .map_err(|e| format!("set version: {e}"))?;
    // The 'static backend txn is fully owned by the handle (no borrows).
    let mut meta = db_handle.metadata().clone();
    meta.version = new_version;
    let handle = TxnHandle {
        txn_id,
        conn_id: open.conn_id,
        db_name: open.name.clone(),
        mode: TxnMode::VersionChange,
        scope: Vec::new(),
        durability: boa_idb_core::proto::Durability::Default,
        meta,
        backend: Some(backend),
        finished: false,
        active: true,
        active_epoch: d.task_epoch,
        explicit_commit: false,
        deleted_store_ids: Vec::new(),
        keygens: HashMap::new(),
    };
    d.scheduler.enqueue(TxnQueueItem {
        id: txn_id,
        mode: TxnMode::VersionChange,
        scope: Vec::new(),
    });
    d.txns.insert(txn_id, handle);

    let scope: Vec<u64> = d
        .txns
        .get(&txn_id)
        .map_or(Vec::new(), |h| h.meta.stores.iter().map(|s| s.id).collect());
    let txn_obj = IdBTransaction::from_data(
        IdBTransaction::new(txn_id, TxnMode::VersionChange, JsObject::with_null_proto()),
        context,
    )
    .map_err(|e| format!("txn from_data: {e}"))?;
    if let Some(mut data) = txn_obj.downcast_mut::<IdBTransaction>() {
        data.txn_id = txn_id;
        data.mode = TxnMode::VersionChange;
        data.durability = boa_idb_core::proto::Durability::Default;
        data.active = true;
        data.scope = scope;
    }
    crate::runtime::register_txn(context, txn_id, txn_obj.clone());

    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        let db_obj = make_db_object(context, &open.name, open.conn_id, new_version, Some(txn_id));
        crate::api::request::with_request_mut(&request, |req| {
            req.result = Some(JsValue::from(db_obj));
            req.ready_state = ReadyState::Done;
            req.transaction = Some(txn_obj.clone());
            req.upgrade_txn = Some(txn_obj.clone());
        });
        // The connection's database object observes the upgrade transaction.
        let db_value =
            crate::api::request::with_request_ref(&request, |req| req.result.clone()).flatten();
        if let Some(db_obj) = db_value.and_then(|v| v.as_object()) {
            if let Some(mut db_data) = db_obj.downcast_mut::<crate::api::database::IdBDatabase>() {
                db_data.upgrade_txn_id = Some(txn_id);
                db_data.version = new_version;
            }
        }
    }
    Ok(txn_id)
}

/// Builds a real `IDBVersionChangeEvent` value.
fn make_versionchange_event(
    context: &mut Context,
    event_type: &str,
    old_version: u64,
    new_version: Option<u64>,
    target: Option<JsObject>,
) -> JsValue {
    let data = crate::api::version_change_event::IdBVersionChangeEventData::new(
        event_type.to_string(),
        old_version,
        new_version,
        target,
    );
    match crate::api::version_change_event::IdBVersionChangeEventData::from_data(data, context) {
        Ok(obj) => JsValue::from(obj),
        Err(_) => JsValue::undefined(),
    }
}

/// Aborts a just-started upgrade whose `upgradeneeded` handler threw.
///
/// The backend transaction rolls back (version + schema), the open request
/// fails with `AbortError`, and the core queue returns to idle.
fn abort_failed_upgrade(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
    open: &PendingOpen,
    txn_id: u64,
) {
    reset_aborted_upgrade_request(context, open);
    // Take + abort the backend (scoped locks, pure).
    let taken = {
        let mut d = crate::runtime::lock_mutex(driver_handle);
        d.txns.remove(&txn_id)
    };
    if let Some(mut handle) = taken {
        if let Some(backend) = handle.backend.take() {
            let _ = backend.abort();
        }
    }
    {
        let mut d = crate::runtime::lock_mutex(driver_handle);
        d.scheduler.forget(txn_id);
        d.txn_queues.remove(&txn_id);
        d.txn_of_request.retain(|_, t| *t != txn_id);
        d.opens.retain(|o| o.request_id != open.request_id);
        if let Some(queue) = d.open_queues.get_mut(&open.name) {
            queue.on_operation_failed();
        }
    }
    // Abort event on the upgrade transaction (no guards held).
    if let Some(txn_obj) = crate::runtime::txn_object(context, txn_id) {
        if let Some(mut data) = txn_obj.downcast_mut::<IdBTransaction>() {
            data.active = false;
            data.finished = true;
        }
        let event = make_event(context, "abort", Some(txn_obj.clone()));
        let _ = dispatch_target(context, &txn_obj, &event);
    }
    crate::runtime::unregister_txn(context, txn_id);
    let _ = fail_open(context, open, &IdbError::Abort);
}

/// Fires the `upgradeneeded` event on the open request.
///
/// The event is a real `IDBVersionChangeEvent` carrying old/new versions.
/// A throwing handler propagates: the caller aborts the upgrade.
fn fire_upgradeneeded(context: &mut Context, open: &PendingOpen) -> JsResult<()> {
    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        let (old, new) = open.upgrade_versions.unwrap_or((0, open.version));
        let event = make_versionchange_event(
            context,
            "upgradeneeded",
            old,
            Some(new),
            Some(request.clone()),
        );
        dispatch_request(context, &request, &event)?;
    }
    Ok(())
}

/// Fires the `blocked` event on the open request.
fn fire_blocked(context: &mut Context, open: &PendingOpen) -> JsResult<()> {
    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        let event = make_event(context, "blocked", Some(request.clone()));
        let _ = dispatch_request(context, &request, &event);
    }
    Ok(())
}

/// Sets the open request result to its database object.
fn set_open_db_result(context: &mut Context, open: &PendingOpen, version: u64) -> JsResult<()> {
    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        let db_obj = make_db_object(context, &open.name, open.conn_id, version, None);
        crate::api::request::with_request_mut(&request, |req| {
            req.result = Some(JsValue::from(db_obj));
            req.ready_state = ReadyState::Done;
        });
    }
    Ok(())
}

/// Builds an `IDBDatabase` object for an open.
fn make_db_object(
    context: &mut Context,
    name: &str,
    conn_id: u64,
    version: u64,
    upgrade_txn_id: Option<u64>,
) -> JsObject {
    let data = crate::api::database::IdBDatabase::new(conn_id, name.to_string(), version);
    let mut data = data;
    data.upgrade_txn_id = upgrade_txn_id;
    crate::api::database::IdBDatabase::from_data(data, context)
        .unwrap_or_else(|_| JsObject::with_null_proto())
}

/// Dispatches the success event for a completed open request.
fn complete_open_success(context: &mut Context, open: &PendingOpen) -> JsResult<()> {
    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        crate::api::request::with_request_mut(&request, |req| {
            req.ready_state = ReadyState::Done;
        });
        let event = make_event(context, "success", Some(request.clone()));
        let _ = dispatch_request(context, &request, &event);
    }
    Ok(())
}

/// Dispatches the success event for a completed delete request.
///
/// Per spec the event is an `IDBVersionChangeEvent` with `newVersion = null`.
fn complete_delete_success(
    context: &mut Context,
    open: &PendingOpen,
    old_version: u64,
) -> JsResult<()> {
    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        crate::api::request::with_request_mut(&request, |req| {
            req.ready_state = ReadyState::Done;
        });
        let event =
            make_versionchange_event(context, "success", old_version, None, Some(request.clone()));
        let _ = dispatch_request(context, &request, &event);
    }
    Ok(())
}

/// Fails a database open with an error event.
fn fail_open(context: &mut Context, open: &PendingOpen, error: &IdbError) -> JsResult<()> {
    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        if dom_exception_value(error, context).is_ok() {
            let err_val = dom_exception_value(error, context)?;
            crate::api::request::with_request_mut(&request, |req| {
                req.ready_state = ReadyState::Done;
                req.error = Some(err_val);
            });
        } else {
            crate::api::request::with_request_mut(&request, |req| {
                req.ready_state = ReadyState::Done;
            });
        }
        let event = make_event(context, "error", Some(request.clone()));
        let _ = dispatch_request(context, &request, &event);
    }
    Ok(())
}

/// Maps an `IdbError` to a DOMException value.
pub(crate) fn dom_exception_value(error: &IdbError, context: &mut Context) -> JsResult<JsValue> {
    let name = match error {
        IdbError::Constraint(_) => "ConstraintError",
        IdbError::TransactionInactive => "TransactionInactiveError",
        IdbError::DataClone(_) => "DataCloneError",
        IdbError::InvalidAccess(_) => "InvalidAccessError",
        IdbError::InvalidState(_) => "InvalidStateError",
        IdbError::NotFound(_) => "NotFoundError",
        IdbError::ReadOnly => "ReadOnlyError",
        IdbError::Abort => "AbortError",
        IdbError::Syntax(_) => "SyntaxError",
        IdbError::Data(_) => "DataError",
        IdbError::Version(_) => "VersionError",
        IdbError::QuotaExceeded { .. } => "QuotaExceededError",
        _ => "UnknownError",
    };
    crate::dom::exception::create_dom_exception(name, &error.to_string(), context)
}

/// Fails every request still queued on a transaction (abort path).
///
/// Dispatch runs with no driver guard held.
fn fail_queued_requests(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
    txn_id: u64,
    error: &IdbError,
) {
    let ids: Vec<u64> = {
        let mut d = crate::runtime::lock_mutex(driver_handle);
        let ids: Vec<u64> = d
            .txn_queues
            .get(&txn_id)
            .map(|q| q.iter().copied().collect())
            .unwrap_or_default();
        for request_id in &ids {
            d.ops.remove(request_id);
            d.txn_of_request.remove(request_id);
        }
        d.txn_queues.remove(&txn_id);
        ids
    };
    for request_id in ids {
        fail_op(driver_handle, context, txn_id, request_id, error.clone());
    }
}

/// One executable transaction-queue unit.
enum TxnWork {
    /// Execute a single queued op.
    Exec {
        txn_id: u64,
        request_id: u64,
        op: PendingOp,
    },
    /// Drain a dead queue (unknown/finished handle) with failures.
    FailStalled { txn_id: u64 },
}

/// Processes all transaction request queues (one op per started txn per turn).
/// Returns progress. Locks are never held across event dispatch.
///
/// Each transaction is served at most once per call: a self-replenishing
/// queue (e.g. a `keep_alive` spin whose `onsuccess` enqueues the next
/// request synchronously) must not keep `drive_turn` from returning, or
/// the event loop could never yield to promise jobs and timers.
#[allow(clippy::too_many_lines)]
fn process_txn_requests(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
) -> JsResult<bool> {
    let mut progressed = false;
    let mut served = std::collections::BTreeSet::new();
    loop {
        // A. Pure dequeue step (locks held, no JS).
        let work: Option<TxnWork> = {
            let mut d = crate::runtime::lock_mutex(driver_handle);
            let mut found = None;
            for txn_id in d.txn_queues.keys().copied().collect::<Vec<_>>() {
                if !served.insert(txn_id) {
                    // Already served this turn: yield so the event loop can
                    // run jobs and timers before this queue is served again.
                    continue;
                }
                match d.txns.get(&txn_id) {
                    Some(t) if !t.finished && t.backend.is_some() => {
                        if let Some(request_id) =
                            d.txn_queues.get_mut(&txn_id).and_then(VecDeque::pop_front)
                        {
                            if let Some(op) = d.ops.remove(&request_id) {
                                d.txn_of_request.remove(&request_id);
                                found = Some(TxnWork::Exec {
                                    txn_id,
                                    request_id,
                                    op,
                                });
                                break;
                            }
                        }
                    }
                    Some(_) => {
                        // Not started yet (scheduler gate): wait for a later turn.
                    }
                    None => {
                        found = Some(TxnWork::FailStalled { txn_id });
                        break;
                    }
                }
            }
            // A finished handle with a leftover queue can never run.
            if found.is_none() {
                if let Some(txn_id) = d
                    .txn_queues
                    .keys()
                    .copied()
                    .find(|id| d.txns.get(id).is_some_and(|t| t.finished))
                {
                    found = Some(TxnWork::FailStalled { txn_id });
                }
            }
            found
        };
        let Some(work) = work else { break };
        match work {
            TxnWork::FailStalled { txn_id } => {
                fail_queued_requests(
                    driver_handle,
                    context,
                    txn_id,
                    &IdbError::TransactionInactive,
                );
                progressed = true;
            }
            TxnWork::Exec {
                txn_id,
                request_id,
                op,
            } => {
                // B. Pure execution (locks held, no JS: backend never calls out).
                let outcome = {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    execute_op(&mut d, txn_id, request_id, &op)
                };
                // C. Completion + dispatch (no guards held).
                match outcome {
                    Ok(OpExec::Outcome(outcome)) => {
                        apply_outcome(request_id, outcome, context);
                        if let PendingOp::CursorOp { cursor_id, .. } = op {
                            set_cursor_pending(context, cursor_id, false);
                        }
                        complete_request(driver_handle, txn_id, request_id, context);
                    }
                    Ok(OpExec::CursorOpened { cursor_id }) => {
                        complete_cursor_open(driver_handle, txn_id, request_id, cursor_id, context);
                    }
                    Ok(OpExec::CursorContinued {
                        cursor_id,
                        reached_end,
                    }) => {
                        complete_cursor_continue(
                            driver_handle,
                            txn_id,
                            request_id,
                            cursor_id,
                            reached_end,
                            context,
                        );
                    }
                    Err(e) => {
                        // A `preventDefault()`d request error does not abort (§2.8).
                        if !fail_op(driver_handle, context, txn_id, request_id, e) {
                            abort_transaction(driver_handle, txn_id, context, false);
                        }
                    }
                }
                progressed = true;
            }
        }
    }
    Ok(progressed)
}

/// Creates a normal (non-upgrade) transaction handle and registers it.
///
/// The backend transaction is started lazily by the driver once the
/// scheduler allows it. Pure state operation: the caller wires the JS object.
pub fn create_txn(
    engine: &mut IdbEngine,
    d: &mut DriverState,
    conn_id: u64,
    db_name: String,
    mode: TxnMode,
    scope: Vec<StoreId>,
    durability: boa_idb_core::proto::Durability,
) -> Result<TxnId, IdbError> {
    let txn_id = d.alloc_txn();
    // Snapshot the schema: versionchange is exclusive, so it cannot change
    // under a live normal transaction.
    let meta = engine.open_db_handle(&db_name).map_or_else(
        |_| boa_idb_core::backend::types::DatabaseMeta {
            name: boa_idb_core::key::utf16::Utf16String::from(db_name.as_str()),
            version: 0,
            stores: Vec::new(),
            next_store_id: 1,
            next_index_id: 1,
        },
        |h| h.metadata().clone(),
    );
    // Validate the scope against the snapshot.
    for sid in &scope {
        if !meta.stores.iter().any(|s| s.id == *sid && !s.deleted) {
            return Err(IdbError::NotFound(format!(
                "Object store id {sid} not found"
            )));
        }
    }
    d.scheduler.enqueue(TxnQueueItem {
        id: txn_id,
        mode,
        scope: scope.clone(),
    });
    d.txns.insert(
        txn_id,
        TxnHandle {
            txn_id,
            conn_id,
            db_name,
            mode,
            scope,
            durability,
            meta,
            backend: None,
            finished: false,
            active: true,
            active_epoch: d.task_epoch,
            explicit_commit: false,
            deleted_store_ids: Vec::new(),
            keygens: HashMap::new(),
        },
    );
    Ok(txn_id)
}

/// Starts backend transactions the scheduler released. Returns progress.
///
/// Pure state work (no dispatch): locks are held throughout.
fn start_ready_txns(
    engine_handle: &Arc<std::sync::Mutex<Option<IdbEngine>>>,
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
) -> JsResult<bool> {
    let mut progressed = false;
    // Drain the ready list; starting one transaction can release the next.
    loop {
        // Scheduler poll (driver lock).
        let ready = {
            let mut d = crate::runtime::lock_mutex(driver_handle);
            d.scheduler.poll_ready()
        };
        if ready.is_empty() {
            break;
        }
        let mut started_any = false;
        for txn_id in ready {
            // Read params (driver lock).
            let params = {
                let d = crate::runtime::lock_mutex(driver_handle);
                match d.txns.get(&txn_id) {
                    Some(t) if !t.finished && t.backend.is_none() => {
                        Some((t.db_name.clone(), t.mode, t.scope.clone(), t.durability))
                    }
                    _ => None,
                }
            };
            let Some((db_name, mode, scope, durability)) = params else {
                continue;
            };
            // Open + begin (engine lock; pure backend IO). Errors are kept
            // (not swallowed) to tell `Locked` apart from fatal failures.
            let backend = {
                let mut engine_guard = crate::runtime::lock_mutex(engine_handle);
                let Some(engine) = engine_guard.as_mut() else {
                    break;
                };
                match engine.open_db_handle(&db_name) {
                    Ok(mut h) => h.begin(mode, &scope, durability),
                    Err(e) => Err(boa_idb_core::backend::error::BackendError::Internal(
                        format!("Failed to open database handle: {e}"),
                    )),
                }
            };
            // Install, retry, or fail (driver lock).
            match backend {
                Ok(b) => {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    if let Some(handle) = d.txns.get_mut(&txn_id) {
                        handle.backend = Some(b);
                    }
                    started_any = true;
                }
                // A taken writer slot is transient under the single-threaded
                // pump (the holder always progresses or finishes): leave the
                // transaction scheduled and retry on a later turn instead of
                // failing its queue. Anything else is fatal.
                Err(boa_idb_core::backend::error::BackendError::Locked) => {}
                Err(_) => {
                    let mut d = crate::runtime::lock_mutex(driver_handle);
                    d.scheduler.forget(txn_id);
                    if let Some(handle) = d.txns.get_mut(&txn_id) {
                        handle.finished = true;
                    }
                    drop(d);
                    fail_queued_requests(
                        driver_handle,
                        context,
                        txn_id,
                        &IdbError::Unknown("Failed to start backend transaction".into()),
                    );
                    progressed = true;
                }
            }
        }
        progressed |= started_any;
        if !started_any {
            break;
        }
    }
    Ok(progressed)
}

/// Finishes drained non-versionchange transactions (auto-commit).
/// Returns progress. Locks are never held across event dispatch.
fn finish_txns(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
) -> JsResult<bool> {
    let mut progressed = false;
    loop {
        // Take one finished-eligible handle out (locks held, pure).
        // Eligible transactions finish in creation (`txn_id`) order: same-
        // scope transactions commit in program order (§2.7.2), which the
        // WPT commit-ordering tests assert.
        let taken: Option<TxnHandle> = {
            let mut d = crate::runtime::lock_mutex(driver_handle);
            let ready = d
                .txns
                .iter()
                .filter(|(k, t)| {
                    !t.finished
                        && t.backend.is_some()
                        && t.mode != TxnMode::VersionChange
                        && (!t.active || t.explicit_commit)
                        && d.txn_queues.get(k).is_none_or(VecDeque::is_empty)
                })
                .map(|(k, _)| *k)
                .min();
            ready.and_then(|txn_id| d.txns.remove(&txn_id))
        };
        let Some(mut handle) = taken else { break };
        let txn_id = handle.txn_id;
        // Commit lock-free (owned backend).
        if let Some(mut backend) = handle.backend.take() {
            for store_id in &handle.deleted_store_ids {
                let _ = backend.delete_store(*store_id);
            }
            let _ = backend.commit();
        }
        // Bookkeeping (locks held, pure).
        {
            let mut d = crate::runtime::lock_mutex(driver_handle);
            d.scheduler.forget(txn_id);
            d.txn_queues.remove(&txn_id);
            d.txn_of_request.retain(|_, t| *t != txn_id);
        }
        // Dispatch (no guards held).
        if let Some(txn_obj) = crate::runtime::txn_object(context, txn_id) {
            if let Some(mut data) = txn_obj.downcast_mut::<IdBTransaction>() {
                data.active = false;
            }
            let event = make_event(context, "complete", Some(txn_obj.clone()));
            let _ = dispatch_target(context, &txn_obj, &event);
        }
        crate::runtime::unregister_txn(context, txn_id);
        progressed = true;
    }
    Ok(progressed)
}

/// Aborts a transaction (backend abort + `abort` event).
///
/// Requests still queued on the transaction fail with `AbortError`.
/// Locks are never held across event dispatch.
pub(crate) fn abort_transaction(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    txn_id: u64,
    context: &mut Context,
    notify_upgrade_open: bool,
) {
    // Take the handle out (locks held, pure).
    let taken: Option<TxnHandle> = {
        let mut d = crate::runtime::lock_mutex(driver_handle);
        d.txns.remove(&txn_id)
    };
    let Some(mut handle) = taken else { return };
    let is_upgrade = handle.mode == TxnMode::VersionChange;
    // Abort lock-free (owned backend).
    if let Some(backend) = handle.backend.take() {
        let _ = backend.abort();
    }
    // Bookkeeping (locks held, pure).
    {
        let mut d = crate::runtime::lock_mutex(driver_handle);
        d.scheduler.forget(txn_id);
        d.txn_queues.remove(&txn_id);
        d.txn_of_request.retain(|_, t| *t != txn_id);
    }
    // Fail queued requests (dispatch, no guards held).
    fail_queued_requests(driver_handle, context, txn_id, &IdbError::Abort);
    // Abort event (dispatch, no guards held).
    if let Some(txn_obj) = crate::runtime::txn_object(context, txn_id) {
        if let Some(mut data) = txn_obj.downcast_mut::<IdBTransaction>() {
            data.active = false;
            data.finished = true;
        }
        let event = make_event(context, "abort", Some(txn_obj.clone()));
        let _ = dispatch_target(context, &txn_obj, &event);
    }
    crate::runtime::unregister_txn(context, txn_id);
    if is_upgrade {
        // Aborting the upgrade fails the open with `AbortError` (§2.9):
        // without this the drained open would commit and succeed.
        abort_upgrade_open(driver_handle, context, txn_id, notify_upgrade_open);
    }
}

/// Fails the open gated by an aborted upgrade transaction.
///
/// Removes the open, releases the core open queue, clears the connection's
/// upgrade marker, and fires `error` (`AbortError`) on the open request.
/// No-op when no open references the transaction (already finished).
fn abort_upgrade_open(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
    txn_id: u64,
    notify_request: bool,
) {
    let open = {
        let mut d = crate::runtime::lock_mutex(driver_handle);
        let position = d
            .opens
            .iter()
            .position(|o| o.upgrade_txn_id == Some(txn_id));
        let Some(position) = position else {
            return;
        };
        let Some(open) = d.opens.remove(position) else {
            return;
        };
        if let Some(queue) = d.open_queues.get_mut(&open.name) {
            queue.on_operation_failed();
        }
        open
    };
    let db_obj = crate::runtime::request_object(context, open.request_id)
        .and_then(|request| {
            crate::api::request::with_request_ref(&request, |req| req.result.clone()).flatten()
        })
        .and_then(|value| value.as_object());
    clear_upgrade_txn(context, &open);
    reset_aborted_upgrade_request(context, &open);
    if let Some(db_obj) = db_obj {
        let event = make_event(context, "abort", Some(db_obj.clone()));
        let _ = dispatch_target(context, &db_obj, &event);
    }
    if notify_request {
        let _ = fail_open(context, &open, &IdbError::Abort);
    } else if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        crate::api::request::with_request_mut(&request, |req| {
            req.ready_state = ReadyState::Done;
            req.error = None;
        });
    }
}

/// Restores the database object and request state after an aborted upgrade.
fn reset_aborted_upgrade_request(context: &mut Context, open: &PendingOpen) {
    let Some(request) = crate::runtime::request_object(context, open.request_id) else {
        return;
    };
    if let Some((old_version, _)) = open.upgrade_versions {
        let db_value =
            crate::api::request::with_request_ref(&request, |req| req.result.clone()).flatten();
        if let Some(db_obj) = db_value.and_then(|value| value.as_object())
            && let Some(mut db_data) = db_obj.downcast_mut::<crate::api::database::IdBDatabase>()
        {
            db_data.version = old_version;
            db_data.upgrade_txn_id = None;
        }
    }
    crate::api::request::with_request_mut(&request, |req| {
        req.result = None;
        req.transaction = None;
        req.upgrade_txn = None;
    });
}

/// Clears the result and transaction of an open request that cannot succeed
/// after its database connection was closed during upgrade.
fn clear_open_request_result(context: &mut Context, open: &PendingOpen) {
    if let Some(request) = crate::runtime::request_object(context, open.request_id) {
        crate::api::request::with_request_mut(&request, |req| {
            req.result = None;
            req.transaction = None;
            req.upgrade_txn = None;
        });
    }
}

/// Clears the connection database object's upgrade-transaction marker.
///
/// Called when the upgrade finishes, aborts, or otherwise stops being the
/// connection's versionchange transaction, so later `createObjectStore` /
/// `deleteObjectStore` calls correctly throw `InvalidStateError` outside an
/// upgrade.
fn clear_upgrade_txn(context: &mut Context, open: &PendingOpen) {
    let db_obj = crate::runtime::request_object(context, open.request_id).and_then(|request| {
        crate::api::request::with_request_ref(&request, |req| req.result.clone()).flatten()
    });
    if let Some(db_obj) = db_obj.and_then(|v| v.as_object()) {
        if let Some(mut db_data) = db_obj.downcast_mut::<crate::api::database::IdBDatabase>() {
            db_data.upgrade_txn_id = None;
        }
    }
}

/// Reads back whether `preventDefault()` was called on a dispatched event.
fn event_default_prevented(event: &JsValue) -> bool {
    let Some(obj) = event.as_object() else {
        return false;
    };
    obj.downcast_ref::<crate::dom::event::EventDataHelper>()
        .is_some_and(|helper| helper.data.default_prevented)
}

/// Fails a request with an `IdbError`, setting the request error value.
///
/// Returns `true` when the `error` event was default-prevented: the caller
/// must then NOT abort the transaction (§2.8: handled request errors).
#[allow(clippy::needless_pass_by_value)]
fn fail_op(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    context: &mut Context,
    txn_id: TxnId,
    request_id: u64,
    error: IdbError,
) -> bool {
    refresh_txn_epoch(driver_handle, txn_id);
    if let Some(request) = crate::runtime::request_object(context, request_id) {
        match dom_exception_value(&error, context) {
            Ok(err_val) => crate::api::request::with_request_mut(&request, |req| {
                req.ready_state = ReadyState::Done;
                req.error = Some(err_val);
            }),
            Err(_) => crate::api::request::with_request_mut(&request, |req| {
                req.ready_state = ReadyState::Done;
            }),
        };
        let event = make_event(context, "error", Some(request.clone()));
        let _ = dispatch_request(context, &request, &event);
        let request_prevented = event_default_prevented(&event);
        // The transaction observes every request failure through its own
        // `error` event (this is what `transactionWatcher` waits for).
        // Canceling either event keeps the transaction alive.
        let txn_prevented = if let Some(txn_obj) = crate::runtime::txn_object(context, txn_id) {
            let txn_event = make_event(context, "error", Some(txn_obj.clone()));
            let _ = dispatch_target(context, &txn_obj, &txn_event);
            event_default_prevented(&txn_event)
        } else {
            false
        };
        return request_prevented || txn_prevented;
    }
    false
}

/// Completes a request as success.
///
/// A throwing `success` listener aborts the transaction (§5.10): the error
/// propagates from `dispatch_request` and the caller aborts.
fn complete_request(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    txn_id: u64,
    request_id: u64,
    context: &mut Context,
) {
    refresh_txn_epoch(driver_handle, txn_id);
    if let Some(request) = crate::runtime::request_object(context, request_id) {
        crate::api::request::with_request_mut(&request, |req| {
            req.ready_state = ReadyState::Done;
        });
        let event = make_event(context, "success", Some(request.clone()));
        if dispatch_request(context, &request, &event).is_err() {
            abort_transaction(driver_handle, txn_id, context, true);
        }
    }
}

/// Completes an open-cursor request; result = cursor object or `null`.
fn complete_cursor_open(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    txn_id: u64,
    request_id: u64,
    cursor_id: u64,
    context: &mut Context,
) {
    // Snapshot cursor meta (locks held, pure).
    let meta = {
        let d = crate::runtime::lock_mutex(driver_handle);
        d.cursors
            .get(&cursor_id)
            .map(|c| (c.direction, c.key_only, c.rows.is_empty()))
    };
    let Some((direction, key_only, empty)) = meta else {
        fail_op(
            driver_handle,
            context,
            txn_id,
            request_id,
            IdbError::InvalidState("cursor not found".into()),
        );
        return;
    };
    let Some(request) = crate::runtime::request_object(context, request_id) else {
        return;
    };
    // Build the cursor object (no guards held).
    let data = crate::api::cursor::IdBCursorData {
        cursor_id,
        request_id,
        direction,
        request: Some(request.clone()),
    };
    let cursor_obj = if key_only {
        crate::api::cursor::IdBCursorData::from_data(data, context)
    } else {
        crate::api::cursor::IdBCursorWithValueData::from_data(
            crate::api::cursor::IdBCursorWithValueData(data, boa_gc::GcRefCell::default()),
            context,
        )
    };
    let Ok(cursor_obj) = cursor_obj else {
        fail_op(
            driver_handle,
            context,
            txn_id,
            request_id,
            IdbError::Unknown("Failed to create cursor object".into()),
        );
        return;
    };
    crate::runtime::register_cursor(context, cursor_id, cursor_obj.clone());
    refresh_txn_epoch(driver_handle, txn_id);
    crate::api::request::with_request_mut(&request, |req| {
        req.ready_state = ReadyState::Done;
        // Empty range: result is null; otherwise the cursor itself (the same
        // object is reused by iteration).
        if empty {
            req.result = Some(JsValue::null());
        } else {
            req.result = Some(JsValue::from(cursor_obj));
        }
    });
    let event = make_event(context, "success", Some(request.clone()));
    if dispatch_request(context, &request, &event).is_err() {
        abort_transaction(driver_handle, txn_id, context, true);
    }
}

/// Completes a cursor-continue request; result = same cursor object or `null`.
fn complete_cursor_continue(
    driver_handle: &Arc<std::sync::Mutex<DriverState>>,
    txn_id: u64,
    request_id: u64,
    cursor_id: u64,
    reached_end: bool,
    context: &mut Context,
) {
    set_cursor_pending(context, cursor_id, false);
    refresh_txn_epoch(driver_handle, txn_id);
    if let Some(request) = crate::runtime::request_object(context, request_id) {
        crate::api::request::with_request_mut(&request, |req| {
            req.ready_state = ReadyState::Done;
            if reached_end {
                req.result = Some(JsValue::null());
            } else if let Some(cursor_obj) = crate::runtime::cursor_object(context, cursor_id) {
                req.result = Some(JsValue::from(cursor_obj));
            } else {
                req.result = Some(JsValue::null());
            }
        });
        let event = make_event(context, "success", Some(request.clone()));
        if dispatch_request(context, &request, &event).is_err() {
            abort_transaction(driver_handle, txn_id, context, true);
        }
    }
}

/// Applies a raw outcome to a request result.
fn apply_outcome(request_id: u64, outcome: RawOutcome, context: &mut Context) {
    let value = outcome_to_js(outcome, context).unwrap_or(JsValue::undefined());
    if let Some(request) = crate::runtime::request_object(context, request_id) {
        crate::api::request::with_request_mut(&request, |req| {
            req.result = Some(value);
        });
    }
}

/// Converts a raw outcome to a JS value.
fn outcome_to_js(outcome: RawOutcome, context: &mut Context) -> JsResult<JsValue> {
    match outcome {
        RawOutcome::Empty => Ok(JsValue::undefined()),
        RawOutcome::Key(k) => key_to_value(&k, context),
        RawOutcome::Value(None) => Ok(JsValue::undefined()),
        RawOutcome::Value(Some(v)) => deserialize_from_storage(&v, context),
        RawOutcome::Keys(keys) => {
            let js: Vec<JsValue> = keys
                .iter()
                .map(|k| key_to_value(k, context))
                .collect::<JsResult<_>>()?;
            Ok(JsArray::from_iter(js, context).into())
        }
        RawOutcome::Values(values) => {
            let js: Vec<JsValue> = values
                .iter()
                .map(|v| deserialize_from_storage(v, context))
                .collect::<JsResult<_>>()?;
            Ok(JsArray::from_iter(js, context).into())
        }
        RawOutcome::Records(rows) => {
            let mut js = Vec::with_capacity(rows.len());
            for row in rows {
                let value = row.value.clone().unwrap_or(ScValue::Null);
                let obj = crate::api::record::create_record_object(
                    row.key.clone(),
                    row.primary_key.clone(),
                    value,
                    context,
                )?;
                js.push(JsValue::from(obj));
            }
            Ok(JsArray::from_iter(js, context).into())
        }
        RawOutcome::Count(c) => Ok(JsValue::from(c)),
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Builds a `{ type, target, currentTarget }` wrapper object.
fn make_event(context: &mut Context, event_type: &str, target: Option<JsObject>) -> JsValue {
    // Request `error` events are cancelable (preventDefault keeps the txn).
    let cancelable = event_type == "error";
    crate::dom::event::create_event_object_cancelable(event_type, cancelable, target, context)
}

/// Dispatches an event on a request (listeners + attribute handlers).
///
/// A throwing listener propagates the error: request completion helpers turn
/// it into a transaction abort (§5.10).
fn dispatch_request(context: &mut Context, request: &JsObject, event: &JsValue) -> JsResult<()> {
    let event_type = event
        .as_object()
        .and_then(|o| o.get(js_string!("type"), context).ok())
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();
    // Snapshot under one borrow; handlers run after it is released and may
    // re-enter (enqueue new requests, remove listeners).
    let snapshots = crate::dom::event_target::snapshot_listeners(request, &event_type);
    let attr = crate::api::request::with_request_ref(request, |d| {
        d.attr_handlers.borrow().get(&event_type).cloned()
    })
    .flatten();

    for (id, callback, once) in snapshots {
        if once {
            crate::dom::event_target::remove_listener_by_id(request, id);
        }
        if let Some(f) = callback.as_callable() {
            f.call(
                &JsValue::from(request.clone()),
                std::slice::from_ref(event),
                context,
            )?;
        }
    }
    if let Some(handler) = attr {
        if let Some(f) = handler.as_callable() {
            f.call(
                &JsValue::from(request.clone()),
                std::slice::from_ref(event),
                context,
            )?;
        }
    }
    Ok(())
}

/// Dispatches an event on a transaction / database object.
///
/// Covers both `addEventListener` registrations and `on*` attributes.
fn dispatch_target(context: &mut Context, target: &JsObject, event: &JsValue) -> JsResult<()> {
    let event_type = event
        .as_object()
        .and_then(|o| o.get(js_string!("type"), context).ok())
        .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
        .unwrap_or_default();

    let snapshots = crate::dom::event_target::snapshot_listeners(target, &event_type);
    let attr = if let Some(data) = target.downcast_ref::<IdBTransaction>() {
        data.attr_handlers.borrow().get(&event_type).cloned()
    } else if let Some(data) = target.downcast_ref::<crate::api::database::IdBDatabase>() {
        data.attr_handlers.borrow().get(&event_type).cloned()
    } else {
        None
    };

    for (id, callback, once) in snapshots {
        if once {
            crate::dom::event_target::remove_listener_by_id(target, id);
        }
        if let Some(f) = callback.as_callable() {
            f.call(
                &JsValue::from(target.clone()),
                std::slice::from_ref(event),
                context,
            )?;
        }
    }
    if let Some(handler) = attr {
        if let Some(f) = handler.as_callable() {
            f.call(
                &JsValue::from(target.clone()),
                std::slice::from_ref(event),
                context,
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Schema operations (synchronous, upgrade transactions only)
// ---------------------------------------------------------------------------
// `createObjectStore` must return the store object synchronously, so schema
// operations execute immediately against the open upgrade backend transaction
// (same atomic unit as the queued work — the upgrade commit covers all).

use boa_idb_core::backend::types::{IndexMeta, StoreMeta};
use boa_idb_core::backend::types::{IndexSpec, StoreSpec};
use boa_idb_core::key::utf16::Utf16String;

/// Requires a live upgrade transaction handle.
fn upgrade_handle_mut(d: &mut DriverState, txn_id: TxnId) -> Result<&mut TxnHandle, IdbError> {
    match d.txns.get_mut(&txn_id) {
        Some(h) if !h.finished && h.mode == TxnMode::VersionChange && h.backend.is_some() => Ok(h),
        Some(_) => Err(IdbError::InvalidState(
            "Schema operations require a live versionchange transaction".into(),
        )),
        None => Err(IdbError::TransactionInactive),
    }
}

/// Resolves a store id by name within a transaction (scope-checked, except
/// for versionchange which sees every store).
pub fn txn_store_id(d: &DriverState, txn_id: TxnId, name: &str) -> Result<StoreId, IdbError> {
    let handle = d
        .txns
        .get(&txn_id)
        .filter(|h| !h.finished)
        .ok_or(IdbError::TransactionInactive)?;
    let uname = Utf16String::from(name);
    handle
        .meta
        .stores
        .iter()
        .filter(|s| !s.deleted)
        .filter(|s| handle.mode == TxnMode::VersionChange || handle.scope.contains(&s.id))
        .find(|s| s.name == uname)
        .map(|s| s.id)
        .ok_or_else(|| IdbError::NotFound(format!("Object store '{name}' not found")))
}

/// Returns a snapshot of a store for handle construction.
pub fn txn_store_view(
    d: &DriverState,
    txn_id: TxnId,
    store_id: StoreId,
) -> Result<StoreMeta, IdbError> {
    d.txns
        .get(&txn_id)
        .filter(|h| !h.finished)
        .and_then(|h| {
            h.meta
                .stores
                .iter()
                // A request queued before deleteObjectStore must still run;
                // the handle is already marked deleted, while physical
                // removal is deferred until commit.
                .find(|s| s.id == store_id)
                .cloned()
        })
        .ok_or_else(|| IdbError::NotFound(format!("Object store id {store_id} not found")))
}

/// Returns a snapshot of an index for handle construction.
pub fn store_index_view(
    d: &DriverState,
    txn_id: TxnId,
    store_id: StoreId,
    index_name: &str,
) -> Result<IndexMeta, IdbError> {
    let uname = Utf16String::from(index_name);
    d.txns
        .get(&txn_id)
        .filter(|h| !h.finished)
        .and_then(|h| {
            h.meta
                .stores
                .iter()
                .find(|s| s.id == store_id && !s.deleted)
        })
        .and_then(|s| {
            s.indexes
                .iter()
                .find(|i| !i.deleted && i.name == uname)
                .cloned()
        })
        .ok_or_else(|| IdbError::NotFound(format!("Index '{index_name}' not found")))
}

/// Lists non-deleted index names of a store.
pub fn store_index_names(d: &DriverState, txn_id: TxnId, store_id: StoreId) -> Vec<String> {
    d.txns
        .get(&txn_id)
        .map(|h| {
            h.meta
                .stores
                .iter()
                .find(|s| s.id == store_id && !s.deleted)
                .map(|s| {
                    s.indexes
                        .iter()
                        .filter(|i| !i.deleted)
                        .map(|i| i.name.to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Lists non-deleted store names visible to a database connection.
///
/// For upgrade connections this reflects the transaction's evolving schema
/// snapshot; otherwise the last committed backend metadata.
pub fn db_store_names(
    engine: &mut IdbEngine,
    d: &DriverState,
    db_name: &str,
    upgrade_txn_id: Option<TxnId>,
) -> Vec<String> {
    if let Some(txn_id) = upgrade_txn_id {
        if let Some(handle) = d.txns.get(&txn_id) {
            if !handle.finished {
                return handle
                    .meta
                    .stores
                    .iter()
                    .filter(|s| !s.deleted)
                    .map(|s| s.name.to_string())
                    .collect();
            }
        }
    }
    engine
        .open_db_handle(db_name)
        .map(|h| {
            h.metadata()
                .stores
                .iter()
                .filter(|s| !s.deleted)
                .map(|s| s.name.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Creates an object store in the upgrade transaction.
pub fn schema_create_store(
    d: &mut DriverState,
    txn_id: TxnId,
    name: &str,
    key_path: Option<KeyPath>,
    auto_increment: bool,
) -> Result<StoreMeta, IdbError> {
    // `autoIncrement` with an *explicit* empty-string or array keyPath is
    // invalid; an *omitted* keyPath (out-of-line keys) is the classic valid
    // auto-increment store.
    if auto_increment
        && key_path
            .as_ref()
            .is_some_and(|kp| !matches!(kp, KeyPath::Single(_)))
    {
        return Err(IdbError::InvalidAccess(
            "autoIncrement requires a single non-empty keyPath".into(),
        ));
    }
    let kp = key_path.unwrap_or(KeyPath::Empty);
    let uname = Utf16String::from(name);
    let handle = upgrade_handle_mut(d, txn_id)?;
    if handle
        .meta
        .stores
        .iter()
        .any(|s| !s.deleted && s.name == uname)
    {
        return Err(IdbError::Constraint(format!(
            "Object store '{name}' already exists"
        )));
    }
    let spec = StoreSpec {
        name: uname.clone(),
        key_path: kp.clone(),
        auto_increment,
    };
    let backend: &mut dyn BackendTxn = handle
        .backend
        .as_mut()
        .map(|b| b.as_mut())
        .ok_or(IdbError::TransactionInactive)?;
    let id = backend.create_store(&spec).map_err(backend_err)?;
    let meta = StoreMeta {
        id,
        name: uname,
        key_path: kp,
        auto_increment,
        key_gen: 1.0,
        indexes: Vec::new(),
        deleted: false,
    };
    let handle = upgrade_handle_mut(d, txn_id)?;
    if id >= handle.meta.next_store_id {
        handle.meta.next_store_id = id + 1;
    }
    handle.meta.stores.push(meta.clone());
    Ok(meta)
}

/// Deletes an object store in the upgrade transaction (records and index
/// entries go through the store algorithm so nothing is orphaned).
pub fn schema_delete_store(d: &mut DriverState, txn_id: TxnId, name: &str) -> Result<(), IdbError> {
    let uname = Utf16String::from(name);
    let store_id = {
        let handle = upgrade_handle_mut(d, txn_id)?;
        handle
            .meta
            .stores
            .iter()
            .find(|s| !s.deleted && s.name == uname)
            .map(|s| s.id)
            .ok_or_else(|| IdbError::NotFound(format!("Object store '{name}' not found")))?
    };
    // Keep the old backend store alive for requests that were queued before
    // deleteObjectStore(), but release its public name immediately so the
    // upgrade may recreate a store with the same name. The old store is
    // physically removed at upgrade commit.
    {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let backend = handle
            .backend
            .as_mut()
            .map(|b| b.as_mut())
            .ok_or(IdbError::TransactionInactive)?;
        let tombstone = format!("\u{0}boa-deleted-{txn_id}-{store_id}");
        backend
            .rename_store(store_id, &tombstone)
            .map_err(backend_err)?;
    }
    let handle = upgrade_handle_mut(d, txn_id)?;
    if let Some(s) = handle.meta.stores.iter_mut().find(|s| s.id == store_id) {
        s.deleted = true;
    }
    handle.deleted_store_ids.push(store_id);
    Ok(())
}

/// Renames an object store in the upgrade transaction.
pub fn schema_rename_store(
    d: &mut DriverState,
    txn_id: TxnId,
    old_name: &str,
    new_name: &str,
) -> Result<(), IdbError> {
    let old_u = Utf16String::from(old_name);
    let new_u = Utf16String::from(new_name);
    let handle = upgrade_handle_mut(d, txn_id)?;
    let store_id = handle
        .meta
        .stores
        .iter()
        .find(|s| !s.deleted && s.name == old_u)
        .map(|s| s.id)
        .ok_or_else(|| IdbError::NotFound(format!("Object store '{old_name}' not found")))?;
    if handle
        .meta
        .stores
        .iter()
        .any(|s| !s.deleted && s.name == new_u)
    {
        return Err(IdbError::Constraint(format!(
            "Object store '{new_name}' already exists"
        )));
    }
    let handle = upgrade_handle_mut(d, txn_id)?;
    let backend: &mut dyn BackendTxn = handle
        .backend
        .as_mut()
        .map(|b| b.as_mut())
        .ok_or(IdbError::TransactionInactive)?;
    backend
        .rename_store(store_id, new_name)
        .map_err(backend_err)?;
    let handle = upgrade_handle_mut(d, txn_id)?;
    if let Some(s) = handle.meta.stores.iter_mut().find(|s| s.id == store_id) {
        s.name = new_u;
    }
    Ok(())
}

/// Creates an index in the upgrade transaction, backfilling existing records.
///
/// A `unique` violation in existing data fails with `ConstraintError` (the
/// caller surfaces it; the upgrade transaction aborts through the normal
/// request-error path when the surrounding script handles it, or
/// synchronously here for direct API misuse).
pub fn schema_create_index(
    d: &mut DriverState,
    txn_id: TxnId,
    store_name: &str,
    name: &str,
    key_path: KeyPath,
    unique: bool,
    multi_entry: bool,
) -> Result<IndexMeta, IdbError> {
    if multi_entry && matches!(key_path, KeyPath::Array(_)) {
        return Err(IdbError::InvalidAccess(
            "multiEntry indexes cannot use an array keyPath".into(),
        ));
    }
    let lim = limits();
    let store_u = Utf16String::from(store_name);
    let index_u = Utf16String::from(name);
    let store_id = {
        let handle = upgrade_handle_mut(d, txn_id)?;
        handle
            .meta
            .stores
            .iter()
            .find(|s| !s.deleted && s.name == store_u)
            .map(|s| s.id)
            .ok_or_else(|| IdbError::NotFound(format!("Object store '{store_name}' not found")))?
    };
    {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let store = handle
            .meta
            .stores
            .iter()
            .find(|s| s.id == store_id)
            .ok_or_else(|| IdbError::NotFound("Object store not found".into()))?;
        if store
            .indexes
            .iter()
            .any(|i| !i.deleted && i.name == index_u)
        {
            return Err(IdbError::Constraint(format!(
                "Index '{name}' already exists"
            )));
        }
    }
    let spec = IndexSpec {
        name: index_u.clone(),
        key_path: key_path.clone(),
        unique,
        multi_entry,
    };
    let index_id = {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let backend: &mut dyn BackendTxn = handle
            .backend
            .as_mut()
            .map(|b| b.as_mut())
            .ok_or(IdbError::TransactionInactive)?;
        backend.create_index(store_id, &spec).map_err(backend_err)?
    };
    // Backfill from existing records.
    {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let backend: &mut dyn BackendTxn = handle
            .backend
            .as_mut()
            .map(|b| b.as_mut())
            .ok_or(IdbError::TransactionInactive)?;
        backfill_index(
            backend,
            store_id,
            index_id,
            &key_path,
            multi_entry,
            unique,
            &lim,
        )?;
    }
    let meta = IndexMeta {
        id: index_id,
        store_id,
        name: index_u,
        key_path,
        unique,
        multi_entry,
        deleted: false,
    };
    let handle = upgrade_handle_mut(d, txn_id)?;
    if index_id >= handle.meta.next_index_id {
        handle.meta.next_index_id = index_id + 1;
    }
    if let Some(store) = handle.meta.stores.iter_mut().find(|s| s.id == store_id) {
        store.indexes.push(meta.clone());
    }
    Ok(meta)
}

/// Deletes an index in the upgrade transaction (entries removed first).
pub fn schema_delete_index(
    d: &mut DriverState,
    txn_id: TxnId,
    store_name: &str,
    name: &str,
) -> Result<(), IdbError> {
    let store_u = Utf16String::from(store_name);
    let index_u = Utf16String::from(name);
    let (store_id, index_id) = {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let store = handle
            .meta
            .stores
            .iter()
            .find(|s| !s.deleted && s.name == store_u)
            .ok_or_else(|| IdbError::NotFound(format!("Object store '{store_name}' not found")))?;
        let index = store
            .indexes
            .iter()
            .find(|i| !i.deleted && i.name == index_u)
            .ok_or_else(|| IdbError::NotFound(format!("Index '{name}' not found")))?;
        (store.id, index.id)
    };
    {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let backend: &mut dyn BackendTxn = handle
            .backend
            .as_mut()
            .map(|b| b.as_mut())
            .ok_or(IdbError::TransactionInactive)?;
        // Remove every entry of this index before dropping the schema entry.
        let pkeys = collect_store_pkeys(backend, store_id)?;
        for pk in &pkeys {
            backend
                .index_delete_by_primary(index_id, pk)
                .map_err(backend_err)?;
        }
        backend
            .delete_index(store_id, index_id)
            .map_err(backend_err)?;
    }
    let handle = upgrade_handle_mut(d, txn_id)?;
    if let Some(store) = handle.meta.stores.iter_mut().find(|s| s.id == store_id) {
        if let Some(index) = store.indexes.iter_mut().find(|i| i.id == index_id) {
            index.deleted = true;
        }
    }
    Ok(())
}

/// Renames an index in the upgrade transaction.
pub fn schema_rename_index(
    d: &mut DriverState,
    txn_id: TxnId,
    store_name: &str,
    old_name: &str,
    new_name: &str,
) -> Result<(), IdbError> {
    let store_u = Utf16String::from(store_name);
    let old_u = Utf16String::from(old_name);
    let new_u = Utf16String::from(new_name);
    let (store_id, index_id) = {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let store = handle
            .meta
            .stores
            .iter()
            .find(|s| !s.deleted && s.name == store_u)
            .ok_or_else(|| IdbError::NotFound(format!("Object store '{store_name}' not found")))?;
        if store.indexes.iter().any(|i| !i.deleted && i.name == new_u) {
            return Err(IdbError::Constraint(format!(
                "Index '{new_name}' already exists"
            )));
        }
        let index = store
            .indexes
            .iter()
            .find(|i| !i.deleted && i.name == old_u)
            .ok_or_else(|| IdbError::NotFound(format!("Index '{old_name}' not found")))?;
        (store.id, index.id)
    };
    {
        let handle = upgrade_handle_mut(d, txn_id)?;
        let backend: &mut dyn BackendTxn = handle
            .backend
            .as_mut()
            .map(|b| b.as_mut())
            .ok_or(IdbError::TransactionInactive)?;
        backend
            .rename_index(store_id, index_id, new_name)
            .map_err(backend_err)?;
    }
    let handle = upgrade_handle_mut(d, txn_id)?;
    if let Some(store) = handle.meta.stores.iter_mut().find(|s| s.id == store_id) {
        if let Some(index) = store.indexes.iter_mut().find(|i| i.id == index_id) {
            index.name = new_u;
        }
    }
    Ok(())
}

/// Collects encoded primary keys of every record in a store.
fn collect_store_pkeys(
    backend: &mut dyn BackendTxn,
    store_id: StoreId,
) -> Result<Vec<Vec<u8>>, IdbError> {
    let mut cursor = backend
        .scan(
            SourceRef::Store(store_id),
            &EncodedRange::all(),
            Direction::Next,
            true,
        )
        .map_err(backend_err)?;
    let mut out = Vec::new();
    let mut active = cursor.seek(CursorSeek::First).map_err(backend_err)?;
    while active {
        out.push(cursor.current_primary_key().to_vec());
        active = cursor.step(1).map_err(backend_err)?;
    }
    Ok(out)
}

/// Backfills a freshly created index from existing records.
fn backfill_index(
    backend: &mut dyn BackendTxn,
    store_id: StoreId,
    index_id: IndexId,
    key_path: &KeyPath,
    multi_entry: bool,
    unique: bool,
    lim: &LimitConfig,
) -> Result<(), IdbError> {
    // Collect rows first: the cursor borrows `backend`, so it must be
    // dropped before the `index_put` calls below.
    let rows: Vec<(Vec<u8>, Vec<u8>)> = {
        let mut cursor = backend
            .scan(
                SourceRef::Store(store_id),
                &EncodedRange::all(),
                Direction::Next,
                false,
            )
            .map_err(backend_err)?;
        let mut out = Vec::new();
        let mut active = cursor.seek(CursorSeek::First).map_err(backend_err)?;
        while active {
            out.push((
                cursor.current_primary_key().to_vec(),
                cursor.current_value().unwrap_or(&[]).to_vec(),
            ));
            active = cursor.step(1).map_err(backend_err)?;
        }
        out
    };
    for (pkey, value_bytes) in rows {
        let value = decode_scf(&value_bytes, lim)
            .map_err(|e| IdbError::NotReadable(format!("Stored value is corrupt: {e}")))?;
        if let Some(idx_key) = key_path.extract(&value)? {
            let keys = if multi_entry {
                match &idx_key {
                    Key::Array(arr) => arr.clone(),
                    single => vec![single.clone()],
                }
            } else {
                vec![idx_key]
            };
            // Deduplicate: the same record must not insert the pair twice.
            let mut seen: Vec<Vec<u8>> = Vec::new();
            for key in keys {
                let mut encoded = Vec::new();
                encode_key(&key, &mut encoded, lim).map_err(encode_key_err)?;
                if seen.contains(&encoded) {
                    continue;
                }
                seen.push(encoded.clone());
                backend
                    .index_put(index_id, &encoded, &pkey, unique)
                    .map_err(|e| match e {
                        boa_idb_core::backend::error::BackendError::Constraint(msg) => {
                            IdbError::Constraint(msg)
                        }
                        other => IdbError::Data(format!("Index backfill failed: {other}")),
                    })?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Operation execution
// ---------------------------------------------------------------------------

/// Executes one pending operation against a transaction.
///
/// Each operation runs inside a backend savepoint (AD-7) together with the
/// transaction's key generators: success commits the savepoint, failure rolls
/// it back (restoring both data and generator positions).
fn execute_op(
    d: &mut DriverState,
    txn_id: TxnId,
    request_id: u64,
    op: &PendingOp,
) -> Result<OpExec, IdbError> {
    // Phase A: open savepoints.
    {
        let handle = d
            .txns
            .get_mut(&txn_id)
            .ok_or(IdbError::TransactionInactive)?;
        let backend: &mut dyn BackendTxn = handle
            .backend
            .as_mut()
            .map(|b| b.as_mut())
            .ok_or(IdbError::TransactionInactive)?;
        backend.begin_request().map_err(backend_err)?;
        for keygen in handle.keygens.values_mut() {
            keygen.begin_request();
        }
    }
    // Phase B: run.
    let result = execute_op_inner(d, txn_id, request_id, op);
    // Phase C: commit or roll back savepoints.
    {
        let handle = d
            .txns
            .get_mut(&txn_id)
            .ok_or(IdbError::TransactionInactive)?;
        if result.is_ok() {
            if let Some(backend) = handle.backend.as_mut() {
                backend.commit_request().map_err(backend_err)?;
            }
            for keygen in handle.keygens.values_mut() {
                keygen.commit_request()?;
            }
        } else {
            if let Some(backend) = handle.backend.as_mut() {
                let _ = backend.rollback_request();
            }
            for keygen in handle.keygens.values_mut() {
                let _ = keygen.rollback_request();
            }
        }
    }
    result
}

/// Executes one pending operation (savepoints handled by [`execute_op`]).
#[allow(clippy::too_many_lines)]
fn execute_op_inner(
    d: &mut DriverState,
    txn_id: TxnId,
    request_id: u64,
    op: &PendingOp,
) -> Result<OpExec, IdbError> {
    let lim = limits();
    let meta = d
        .txns
        .get(&txn_id)
        .filter(|t| !t.finished)
        .map(|t| t.meta.clone())
        .ok_or(IdbError::TransactionInactive)?;

    match op {
        PendingOp::Get { source, range } => {
            let value = {
                let handle = d
                    .txns
                    .get_mut(&txn_id)
                    .ok_or(IdbError::TransactionInactive)?;
                let backend: &mut dyn BackendTxn = handle
                    .backend
                    .as_mut()
                    .map(|b| b.as_mut())
                    .ok_or(IdbError::TransactionInactive)?;
                first_value(backend, *source, range, &lim)?
            };
            Ok(OpExec::Outcome(RawOutcome::Value(value)))
        }
        PendingOp::Count { source, range } => {
            let encoded = range.to_encoded(&lim)?;
            let handle = d
                .txns
                .get_mut(&txn_id)
                .ok_or(IdbError::TransactionInactive)?;
            let backend: &mut dyn BackendTxn = handle
                .backend
                .as_mut()
                .map(|b| b.as_mut())
                .ok_or(IdbError::TransactionInactive)?;
            let count = backend.count(*source, &encoded).map_err(backend_err)?;
            Ok(OpExec::Outcome(RawOutcome::Count(count)))
        }
        PendingOp::GetKey { source, range } => {
            let key = {
                let handle = d
                    .txns
                    .get_mut(&txn_id)
                    .ok_or(IdbError::TransactionInactive)?;
                let backend: &mut dyn BackendTxn = handle
                    .backend
                    .as_mut()
                    .map(|b| b.as_mut())
                    .ok_or(IdbError::TransactionInactive)?;
                first_key(backend, *source, range, &lim)?
            };
            match key {
                Some(k) => Ok(OpExec::Outcome(RawOutcome::Key(k))),
                None => Ok(OpExec::Outcome(RawOutcome::Value(None))),
            }
        }
        PendingOp::GetAll {
            source,
            range,
            limit,
            keys_only,
        } => {
            if *keys_only {
                let keys = {
                    let handle = d
                        .txns
                        .get_mut(&txn_id)
                        .ok_or(IdbError::TransactionInactive)?;
                    let backend: &mut dyn BackendTxn = handle
                        .backend
                        .as_mut()
                        .map(|b| b.as_mut())
                        .ok_or(IdbError::TransactionInactive)?;
                    collect_keys(backend, *source, range, *limit, &lim)?
                };
                Ok(OpExec::Outcome(RawOutcome::Keys(keys)))
            } else {
                let values = {
                    let handle = d
                        .txns
                        .get_mut(&txn_id)
                        .ok_or(IdbError::TransactionInactive)?;
                    let backend: &mut dyn BackendTxn = handle
                        .backend
                        .as_mut()
                        .map(|b| b.as_mut())
                        .ok_or(IdbError::TransactionInactive)?;
                    collect_values(backend, *source, range, *limit, &lim)?
                };
                Ok(OpExec::Outcome(RawOutcome::Values(values)))
            }
        }
        PendingOp::GetAllRecords {
            source,
            range,
            limit,
            direction,
        } => {
            let mut rows = {
                let handle = d
                    .txns
                    .get_mut(&txn_id)
                    .ok_or(IdbError::TransactionInactive)?;
                let backend: &mut dyn BackendTxn = handle
                    .backend
                    .as_mut()
                    .map(|b| b.as_mut())
                    .ok_or(IdbError::TransactionInactive)?;
                materialize(backend, *source, range, *direction, false, &lim)?
            };
            if let Some(limit) = limit {
                rows.truncate(*limit as usize);
            }
            Ok(OpExec::Outcome(RawOutcome::Records(rows)))
        }
        PendingOp::Put {
            store_id,
            value,
            explicit_key,
            no_overwrite,
        } => {
            let store_meta = meta
                .stores
                .iter()
                // Requests already queued before deleteObjectStore() retain
                // the store snapshot they were created against. The JS
                // handle itself is invalidated synchronously, but execution
                // of that queued request is still part of the upgrade.
                .find(|s| s.id == *store_id)
                .cloned()
                .ok_or_else(|| IdbError::NotFound("Object store not found".into()))?;
            let handle = d
                .txns
                .get_mut(&txn_id)
                .ok_or(IdbError::TransactionInactive)?;
            let backend: &mut dyn BackendTxn = handle
                .backend
                .as_mut()
                .map(|b| b.as_mut())
                .ok_or(IdbError::TransactionInactive)?;
            // Seed the generator from the backend so sequences continue
            // across transactions (the registry snapshot may be stale).
            let base = backend
                .key_gen_current(*store_id)
                .unwrap_or(store_meta.key_gen);
            // A generator created here missed the Phase-A savepoint opening,
            // so open one explicitly to keep begin/commit balanced.
            let keygen = match handle.keygens.entry(*store_id) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let keygen = entry.insert(KeyGenerator::new(base));
                    keygen.begin_request();
                    keygen
                }
            };
            let mut value = value.clone();
            let result = boa_idb_core::engine::ops_store::put(
                backend,
                &store_meta,
                keygen,
                &mut value,
                explicit_key.as_ref(),
                *no_overwrite,
                &lim,
            )?;
            Ok(OpExec::Outcome(RawOutcome::Key(result.key)))
        }
        PendingOp::Delete { store_id, range } => {
            let store_meta = meta
                .stores
                .iter()
                .find(|s| s.id == *store_id)
                .cloned()
                .ok_or_else(|| IdbError::NotFound("Object store not found".into()))?;
            let encoded = range.to_encoded(&lim)?;
            let handle = d
                .txns
                .get_mut(&txn_id)
                .ok_or(IdbError::TransactionInactive)?;
            let backend: &mut dyn BackendTxn = handle
                .backend
                .as_mut()
                .map(|b| b.as_mut())
                .ok_or(IdbError::TransactionInactive)?;
            boa_idb_core::engine::ops_store::delete(backend, &store_meta, &encoded, &lim)?;
            Ok(OpExec::Outcome(RawOutcome::Empty))
        }
        PendingOp::Clear { store_id } => {
            let store_meta = meta
                .stores
                .iter()
                .find(|s| s.id == *store_id)
                .cloned()
                .ok_or_else(|| IdbError::NotFound("Object store not found".into()))?;
            let handle = d
                .txns
                .get_mut(&txn_id)
                .ok_or(IdbError::TransactionInactive)?;
            let backend: &mut dyn BackendTxn = handle
                .backend
                .as_mut()
                .map(|b| b.as_mut())
                .ok_or(IdbError::TransactionInactive)?;
            boa_idb_core::engine::ops_store::clear(backend, &store_meta)?;
            Ok(OpExec::Outcome(RawOutcome::Empty))
        }
        PendingOp::OpenCursor {
            source,
            range,
            direction,
            key_only,
        } => {
            let cursor_id = d.alloc_cursor();
            let rows = {
                let handle = d
                    .txns
                    .get_mut(&txn_id)
                    .ok_or(IdbError::TransactionInactive)?;
                let backend: &mut dyn BackendTxn = handle
                    .backend
                    .as_mut()
                    .map(|b| b.as_mut())
                    .ok_or(IdbError::TransactionInactive)?;
                materialize(backend, *source, range, *direction, *key_only, &lim)?
            };
            let state = CursorState {
                cursor_id,
                txn_id,
                request_id,
                source: *source,
                direction: *direction,
                key_only: *key_only,
                rows,
                pos: 0,
                pending: false,
            };
            d.cursors.insert(cursor_id, state);
            Ok(OpExec::CursorOpened { cursor_id })
        }
        PendingOp::CursorOp { cursor_id, action } => match action {
            CursorAction::Update(sc_value) => {
                let (store_id, pkey, has_key_path) = {
                    let cursor = d
                        .cursors
                        .get(cursor_id)
                        .ok_or_else(|| IdbError::InvalidState("cursor not found".into()))?;
                    let row = cursor
                        .current()
                        .ok_or_else(|| IdbError::InvalidState("cursor at end".into()))?;
                    let store_id = store_of(cursor.source);
                    let has_key_path = meta
                        .stores
                        .iter()
                        .find(|s| s.id == store_id)
                        .is_none_or(|s| !matches!(s.key_path, KeyPath::Empty));
                    (store_id, row.primary_key.clone(), has_key_path)
                };
                let store_meta = meta
                    .stores
                    .iter()
                    .find(|s| s.id == store_id)
                    .cloned()
                    .ok_or_else(|| IdbError::NotFound("Object store not found".into()))?;
                // In-line-keyed stores take the key from the value; passing
                // the cursor key explicitly would (correctly) trip the
                // keyPath/explicit-key check in `ops_store::put`.
                if has_key_path {
                    if let Some(k) = store_meta.key_path.extract(sc_value)? {
                        if k != pkey {
                            return Err(IdbError::Data(
                                "Cursor update changes the in-line key".into(),
                            ));
                        }
                    }
                }
                let explicit = if has_key_path { None } else { Some(pkey) };
                let handle = d
                    .txns
                    .get_mut(&txn_id)
                    .ok_or(IdbError::TransactionInactive)?;
                let backend: &mut dyn BackendTxn = handle
                    .backend
                    .as_mut()
                    .map(|b| b.as_mut())
                    .ok_or(IdbError::TransactionInactive)?;
                let base = backend
                    .key_gen_current(store_id)
                    .unwrap_or(store_meta.key_gen);
                let keygen = match handle.keygens.entry(store_id) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let keygen = entry.insert(KeyGenerator::new(base));
                        keygen.begin_request();
                        keygen
                    }
                };
                let mut value = sc_value.clone();
                let result = boa_idb_core::engine::ops_store::put(
                    backend,
                    &store_meta,
                    keygen,
                    &mut value,
                    explicit.as_ref(),
                    false,
                    &lim,
                )?;
                Ok(OpExec::Outcome(RawOutcome::Key(result.key)))
            }
            CursorAction::Delete => {
                let (store_id, pkey) = {
                    let cursor = d
                        .cursors
                        .get(cursor_id)
                        .ok_or_else(|| IdbError::InvalidState("cursor not found".into()))?;
                    let row = cursor
                        .current()
                        .ok_or_else(|| IdbError::InvalidState("cursor at end".into()))?;
                    (store_of(cursor.source), row.primary_key.clone())
                };
                let store_meta = meta
                    .stores
                    .iter()
                    .find(|s| s.id == store_id)
                    .cloned()
                    .ok_or_else(|| IdbError::NotFound("Object store not found".into()))?;
                let pk = enc_key(&pkey, &lim)?;
                let handle = d
                    .txns
                    .get_mut(&txn_id)
                    .ok_or(IdbError::TransactionInactive)?;
                let backend: &mut dyn BackendTxn = handle
                    .backend
                    .as_mut()
                    .map(|b| b.as_mut())
                    .ok_or(IdbError::TransactionInactive)?;
                // Route through the store algorithm so index entries of the
                // deleted record are removed as well.
                let range = EncodedRange::only(pk);
                boa_idb_core::engine::ops_store::delete(backend, &store_meta, &range, &lim)?;
                Ok(OpExec::Outcome(RawOutcome::Empty))
            }
            _ => {
                let Some(state) = d.cursors.get_mut(cursor_id) else {
                    return Err(IdbError::InvalidState("cursor not found".into()));
                };
                let reached_end = !apply_cursor_action(state, action);
                Ok(OpExec::CursorContinued {
                    cursor_id: *cursor_id,
                    reached_end,
                })
            }
        },
    }
}

/// Reads the first matching record value.
fn first_value(
    backend: &mut dyn BackendTxn,
    source: SourceRef,
    range: &RangeData,
    lim: &LimitConfig,
) -> Result<Option<ScValue>, IdbError> {
    let encoded = range.to_encoded(lim)?;
    // Scope the cursor: it borrows `backend`, so it must be dropped before
    // the point lookup below.
    let pk_bytes = {
        let mut cursor = backend
            .scan(source, &encoded, Direction::Next, true)
            .map_err(backend_err)?;
        if !cursor
            .seek(CursorSeek::First)
            .map_err(backend_err)
            .unwrap_or(false)
        {
            return Ok(None);
        }
        let pk = decode_key(cursor.current_primary_key())
            .map(|(k, _)| k)
            .map_err(encode_key_err)?;
        enc_key(&pk, lim)?
    };
    match backend
        .get(store_of(source), &pk_bytes)
        .map_err(backend_err)?
    {
        Some(bytes) => {
            let sc = decode_scf(&bytes, lim)
                .map_err(|e| IdbError::Data(format!("SCF decode failed: {e}")))?;
            Ok(Some(sc))
        }
        None => Ok(None),
    }
}

/// Reads the first key in a range (getKey).
fn first_key(
    backend: &mut dyn BackendTxn,
    source: SourceRef,
    range: &RangeData,
    lim: &LimitConfig,
) -> Result<Option<Key>, IdbError> {
    let encoded = range.to_encoded(lim)?;
    let mut cursor = backend
        .scan(source, &encoded, Direction::Next, true)
        .map_err(backend_err)?;
    if !cursor
        .seek(CursorSeek::First)
        .map_err(backend_err)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    decode_key(cursor.current_primary_key())
        .map(|(k, _)| Some(k))
        .map_err(encode_key_err)
}

/// Collects primary keys matching a range.
fn collect_keys(
    backend: &mut dyn BackendTxn,
    source: SourceRef,
    range: &RangeData,
    limit: Option<u32>,
    lim: &LimitConfig,
) -> Result<Vec<Key>, IdbError> {
    let encoded = range.to_encoded(lim)?;
    let mut cursor = backend
        .scan(source, &encoded, Direction::Next, true)
        .map_err(backend_err)?;
    let mut out = Vec::new();
    let mut step = cursor.seek(CursorSeek::First).map_err(backend_err)?;
    while step {
        let k = decode_key(cursor.current_primary_key())
            .map(|(k, _)| k)
            .map_err(encode_key_err)?;
        out.push(k);
        if let Some(limit) = limit
            && out.len() >= limit as usize
        {
            break;
        }
        step = cursor.step(1).map_err(backend_err)?;
    }
    Ok(out)
}

/// Collects values matching a range.
fn collect_values(
    backend: &mut dyn BackendTxn,
    source: SourceRef,
    range: &RangeData,
    limit: Option<u32>,
    lim: &LimitConfig,
) -> Result<Vec<ScValue>, IdbError> {
    let encoded = range.to_encoded(lim)?;
    // Collect primary keys first (the cursor borrows `backend`), then fetch
    // each value with point lookups.
    let pks: Vec<Vec<u8>> = {
        let mut cursor = backend
            .scan(source, &encoded, Direction::Next, true)
            .map_err(backend_err)?;
        let mut out = Vec::new();
        let mut step = cursor.seek(CursorSeek::First).map_err(backend_err)?;
        while step {
            let pk = decode_key(cursor.current_primary_key())
                .map(|(k, _)| k)
                .map_err(encode_key_err)?;
            out.push(enc_key(&pk, lim)?);
            if let Some(limit) = limit
                && out.len() >= limit as usize
            {
                break;
            }
            step = cursor.step(1).map_err(backend_err)?;
        }
        out
    };
    let mut out = Vec::with_capacity(pks.len());
    for pk_bytes in pks {
        if let Some(bytes) = backend
            .get(store_of(source), &pk_bytes)
            .map_err(backend_err)?
        {
            let sc = decode_scf(&bytes, lim)
                .map_err(|e| IdbError::Data(format!("SCF decode failed: {e}")))?;
            out.push(sc);
        }
        if let Some(limit) = limit
            && out.len() >= limit as usize
        {
            break;
        }
    }
    Ok(out)
}

/// Materializes cursor rows (keys, primary keys and optionally values).
fn materialize(
    backend: &mut dyn BackendTxn,
    source: SourceRef,
    range: &RangeData,
    direction: Direction,
    key_only: bool,
    lim: &LimitConfig,
) -> Result<Vec<CursorRow>, IdbError> {
    let encoded = range.to_encoded(lim)?;
    let mut cursor = backend
        .scan(source, &encoded, direction, key_only)
        .map_err(backend_err)?;
    let mut rows = Vec::new();
    let mut step = cursor.seek(CursorSeek::First).map_err(backend_err)?;
    while step {
        let key = decode_key(cursor.current_key())
            .map(|(k, _)| k)
            .map_err(encode_key_err)?;
        let pk = decode_key(cursor.current_primary_key())
            .map(|(k, _)| k)
            .map_err(encode_key_err)?;
        let value = if key_only {
            None
        } else {
            cursor
                .current_value()
                .map(|v| {
                    decode_scf(v, lim)
                        .map_err(|e| IdbError::Data(format!("SCF decode failed: {e}")))
                })
                .transpose()?
        };
        rows.push(CursorRow {
            key,
            primary_key: pk,
            value,
        });
        step = cursor.step(1).map_err(backend_err)?;
    }
    Ok(rows)
}
