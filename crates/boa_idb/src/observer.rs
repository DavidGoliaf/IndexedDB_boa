//! Host-side observability for IndexedDB operations.
//!
//! This module provides two complementary mechanisms (§12.3, §4.3):
//!
//! * [`IdbObserver`]: a host-registered callback receiving [`IdbEvent`]s
//!   (`DatabaseOpened`, `TransactionCommitted { duration, bytes_written }`,
//!   `QuotaExceeded`, `Corruption`, …) for integration with host metrics.
//! * Built-in counters and bounded latency histograms ([`IdbStats`]), always
//!   available via [`IdbRuntime::stats`][crate::runtime::IdbRuntime::stats].
//!
//! Plus, under the `tracing` cargo feature (off by default), spans
//! `idb.open`, `idb.txn` and `idb.request` with privacy-safe attributes
//! (database name, mode, scope size, sequence numbers, durations and byte
//! counts — never keys, values, or hashes; see [`PRIVACY_NOTE`]).
//!
//! # Cost model
//!
//! With the `tracing` feature disabled no `tracing` code is compiled in
//! (span fields are `#[cfg]`-gated out of hot structs). The always-on part
//! is a handful of `u64` counter increments plus one `Instant::now` pair per
//! request/transaction — near-zero next to backend IO.
//!
//! # Observer discipline
//!
//! `on_event` runs on the pump (JS thread) with no engine/driver locks held.
//! Observers must be fast, must not touch the Boa [`Context`]
//! (re-entrancy/deadlock risk), must not retain IDB objects or contexts
//! (events carry only plain data), and must not panic: a panic unwinds
//! through the pump into the host's `run_jobs` caller.
//!
//! # Privacy
//!
//! [`PRIVACY_NOTE`]: events and span attributes carry database names, modes,
//! counts, durations and byte totals only. Key bytes, value bytes, storage
//! keys and content hashes are never recorded. `dump --values`-style payload
//! exposure lives only in the dev-only CLI behind an explicit flag.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use boa_engine::Context;
use boa_idb_core::error::IdbError;
use boa_idb_core::proto::TxnMode;

use crate::runtime::IdbRuntime;

/// Privacy contract for every event and span attribute in this module.
///
/// Database *names* are recorded (the work order explicitly lists `database`
/// as a span attribute); key bytes, value bytes and hashes never are.
pub const PRIVACY_NOTE: &str =
    "attributes: database name, mode, counts, durations, byte totals; never keys/values/hashes";

/// Upper bounds (inclusive, microseconds) of the latency histogram buckets.
///
/// The last bucket is the overflow bucket, so the histogram is bounded by
/// construction: exactly [`HISTOGRAM_BUCKETS`] counters.
pub const HISTOGRAM_BOUNDS_US: [u64; 9] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    u64::MAX,
];

/// Number of latency histogram buckets.
pub const HISTOGRAM_BUCKETS: usize = HISTOGRAM_BOUNDS_US.len();

/// Host-visible lifecycle event.
///
/// All variants carry plain data only (no `JsValue`, no IDB objects, no
/// contexts), so observers cannot retain garbage-collected state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdbEvent {
    /// A database connection finished opening.
    DatabaseOpened {
        /// Database name.
        database: String,
        /// Committed version.
        version: u64,
    },
    /// A backend transaction started executing.
    TransactionBegun {
        /// Driver transaction id.
        txn: u64,
        /// Database name.
        database: String,
        /// Transaction mode.
        mode: TxnMode,
        /// Number of stores in scope.
        scope_len: usize,
    },
    /// A backend transaction committed.
    TransactionCommitted {
        /// Driver transaction id.
        txn: u64,
        /// Database name.
        database: String,
        /// Wall time from backend begin to commit.
        duration: Duration,
        /// SCF payload bytes read inside this transaction.
        bytes_read: u64,
        /// SCF payload bytes written inside this transaction.
        bytes_written: u64,
    },
    /// A backend transaction aborted.
    TransactionAborted {
        /// Driver transaction id.
        txn: u64,
        /// Database name.
        database: String,
    },
    /// One queued request finished executing (success or failure).
    RequestCompleted {
        /// Owning driver transaction id.
        txn: u64,
        /// Driver request id.
        request: u64,
        /// Wall time of backend execution.
        duration: Duration,
        /// SCF payload bytes read by this request.
        bytes_read: u64,
        /// SCF payload bytes written by this request.
        bytes_written: u64,
        /// Whether the request failed.
        failed: bool,
    },
    /// A quota check rejected an operation.
    QuotaExceeded {
        /// Database name.
        database: String,
    },
    /// Stored data failed integrity validation on read.
    Corruption {
        /// Database name.
        database: String,
    },
}

/// Host hook for IndexedDB lifecycle events.
///
/// See the [module-level discipline notes](self) before implementing.
pub trait IdbObserver: Send + Sync + 'static {
    /// Observes one lifecycle event. Must be fast, lock-free and infallible.
    fn on_event(&self, event: &IdbEvent);
}

/// Bounded latency histogram with fixed microsecond buckets.
#[derive(Debug, Clone)]
pub struct LatencyHistogram {
    buckets: [u64; HISTOGRAM_BUCKETS],
}

impl LatencyHistogram {
    /// Creates an empty histogram.
    pub fn new() -> Self {
        Self {
            buckets: [0; HISTOGRAM_BUCKETS],
        }
    }

    /// Records one sample.
    pub fn record(&mut self, duration: Duration) {
        let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
        let idx = HISTOGRAM_BOUNDS_US
            .iter()
            .position(|&b| micros <= b)
            .unwrap_or(HISTOGRAM_BUCKETS - 1);
        self.buckets[idx] = self.buckets[idx].saturating_add(1);
    }

    /// Returns the bucket counters (index-aligned with [`HISTOGRAM_BOUNDS_US`]).
    pub fn buckets(&self) -> [u64; HISTOGRAM_BUCKETS] {
        self.buckets
    }

    /// Returns the total number of recorded samples.
    pub fn total(&self) -> u64 {
        self.buckets.iter().sum()
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of the built-in counters and histograms.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdbStats {
    /// Backend transactions started.
    pub txns_begun: u64,
    /// Backend transactions committed.
    pub txns_committed: u64,
    /// Backend transactions aborted.
    pub txns_aborted: u64,
    /// Executed requests completed (success).
    pub requests_completed: u64,
    /// Executed requests failed, plus queued requests dropped by aborts.
    pub requests_failed: u64,
    /// SCF payload bytes read from backends, summed over executed requests.
    ///
    /// Counted exactly once per request, in `RequestCompleted` only: the
    /// per-transaction totals carried by `TransactionCommitted` are observer
    /// data, not a second counting (committed and aborted transactions alike
    /// are covered, since their executed requests all complete first).
    pub bytes_read: u64,
    /// SCF payload bytes written to backends, summed over executed requests.
    ///
    /// Same single-counting rule as `bytes_read`.
    pub bytes_written: u64,
    /// Executed-request latency distribution.
    pub request_latency: [u64; HISTOGRAM_BUCKETS],
    /// Backend-transaction latency distribution (begin → finish).
    pub txn_latency: [u64; HISTOGRAM_BUCKETS],
    /// Registered host observers.
    pub observers: usize,
}

/// Mutable observer/statistics state held by [`IdbRuntime`].
///
/// Plain data only: no JS values, no IDB objects, no contexts.
pub struct ObserverState {
    observers: Vec<Arc<dyn IdbObserver>>,
    txns_begun: u64,
    txns_committed: u64,
    txns_aborted: u64,
    requests_completed: u64,
    requests_failed: u64,
    bytes_read: u64,
    bytes_written: u64,
    request_latency: LatencyHistogram,
    txn_latency: LatencyHistogram,
}

impl ObserverState {
    /// Creates empty state without host observers.
    pub fn new() -> Self {
        Self {
            observers: Vec::new(),
            txns_begun: 0,
            txns_committed: 0,
            txns_aborted: 0,
            requests_completed: 0,
            requests_failed: 0,
            bytes_read: 0,
            bytes_written: 0,
            request_latency: LatencyHistogram::new(),
            txn_latency: LatencyHistogram::new(),
        }
    }

    /// Registers a host observer (called at most once per observer).
    pub fn add_observer(&mut self, observer: Arc<dyn IdbObserver>) {
        self.observers.push(observer);
    }

    /// Records an event: updates built-in counters/histograms and returns the
    /// observer list for lock-free fan-out by the caller.
    ///
    /// Must be called with no engine/driver locks held. The returned clones
    /// let callbacks run with no mutex held at all (P1-1): an observer that
    /// calls back into [`IdbStats`] (or any runtime state) cannot deadlock.
    pub fn record(&mut self, event: &IdbEvent) -> Vec<Arc<dyn IdbObserver>> {
        match event {
            IdbEvent::DatabaseOpened { .. } => {}
            IdbEvent::TransactionBegun { .. } => {
                self.txns_begun = self.txns_begun.saturating_add(1);
            }
            IdbEvent::TransactionCommitted { duration, .. } => {
                // No byte accounting here: the transaction totals ride along
                // for observers, but the global counters are fed by
                // `RequestCompleted` alone — otherwise every committed byte
                // would be counted twice (and aborted bytes once, making the
                // metric outcome-dependent).
                self.txns_committed = self.txns_committed.saturating_add(1);
                self.txn_latency.record(*duration);
            }
            IdbEvent::TransactionAborted { .. } => {
                self.txns_aborted = self.txns_aborted.saturating_add(1);
            }
            IdbEvent::RequestCompleted {
                duration,
                bytes_read,
                bytes_written,
                failed,
                ..
            } => {
                if *failed {
                    self.requests_failed = self.requests_failed.saturating_add(1);
                } else {
                    self.requests_completed = self.requests_completed.saturating_add(1);
                }
                self.request_latency.record(*duration);
                self.bytes_read = self.bytes_read.saturating_add(*bytes_read);
                self.bytes_written = self.bytes_written.saturating_add(*bytes_written);
            }
            IdbEvent::QuotaExceeded { .. } | IdbEvent::Corruption { .. } => {}
        }
        self.observers.clone()
    }

    /// Counts queued requests dropped without executing (abort cascades).
    pub fn record_dropped_requests(&mut self, count: usize) {
        self.requests_failed = self.requests_failed.saturating_add(count as u64);
    }

    /// Snapshots the current counters and histograms.
    pub fn stats(&self) -> IdbStats {
        IdbStats {
            txns_begun: self.txns_begun,
            txns_committed: self.txns_committed,
            txns_aborted: self.txns_aborted,
            requests_completed: self.requests_completed,
            requests_failed: self.requests_failed,
            bytes_read: self.bytes_read,
            bytes_written: self.bytes_written,
            request_latency: self.request_latency.buckets(),
            txn_latency: self.txn_latency.buckets(),
            observers: self.observers.len(),
        }
    }
}

impl Default for ObserverState {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetches the observer state for a context, if IndexedDB is registered.
pub(crate) fn observer_state(context: &Context) -> Option<Arc<Mutex<ObserverState>>> {
    context
        .get_data::<IdbRuntime>()
        .map(|runtime| Arc::clone(&runtime.observer))
}

/// Emits an event when a runtime is present; otherwise drops it silently.
///
/// Every pump hook uses this so observability can never break execution.
/// Counters update under the observer mutex; host callbacks run after the
/// guard is dropped, with no engine/driver/observer lock held (P1-1).
pub(crate) fn emit(context: &mut Context, event: &IdbEvent) {
    let observers = if let Some(state) = observer_state(context) {
        crate::runtime::lock_mutex(&state).record(event)
    } else {
        return;
    };
    for observer in observers {
        observer.on_event(event);
    }
}

/// Error class with a dedicated lifecycle event.
///
/// Backend-level quota/corruption errors are currently flattened into
/// `IdbError::Data` by the `backend_err` mappers, so only the explicit
/// `IdbError` variants surface here. Preserving backend error kinds through
/// the mappers (a JS-visible error-name change) is an M7-B follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorClass {
    /// Storage quota rejected the operation.
    Quota,
    /// Stored data failed integrity validation on read.
    Corruption,
}

/// Classifies a request error for dedicated lifecycle events.
pub(crate) fn classify_error(error: &IdbError) -> Option<ErrorClass> {
    match error {
        IdbError::QuotaExceeded { .. } => Some(ErrorClass::Quota),
        // `NotReadable` is produced when stored payloads fail integrity
        // validation on read (corrupt SCF / torn values).
        IdbError::NotReadable(_) => Some(ErrorClass::Corruption),
        _ => None,
    }
}

/// Emits the dedicated event for a classified request error, if any.
///
/// Details stay static strings: error messages may embed storage internals
/// and must never reach observers or spans verbatim.
pub(crate) fn emit_error_class(context: &mut Context, database: &str, error: &IdbError) {
    match classify_error(error) {
        Some(ErrorClass::Quota) => emit(
            context,
            &IdbEvent::QuotaExceeded {
                database: database.to_owned(),
            },
        ),
        Some(ErrorClass::Corruption) => emit(
            context,
            &IdbEvent::Corruption {
                database: database.to_owned(),
            },
        ),
        None => {}
    }
}

/// Whole milliseconds of a duration, saturating (span attributes).
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Whole microseconds of a duration, saturating (span attributes).
pub(crate) fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Short static error name for the `idb.open` failure attribute.
///
/// Mirrors the `DOMException` mapping without allocating or touching user
/// data; kept as a plain `match` so new variants fail loudly at compile
/// time only if this function is updated — it defaults to `"Error"`.
pub(crate) fn error_name(error: &IdbError) -> &'static str {
    match error {
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
        IdbError::NotReadable(_) => "NotReadableError",
        IdbError::Unknown(_) => "UnknownError",
        _ => "Error",
    }
}

/// Short mode name for span attributes (never leaks user data).
pub(crate) fn txn_mode_name(mode: TxnMode) -> &'static str {
    match mode {
        TxnMode::ReadOnly => "readonly",
        TxnMode::ReadWrite => "readwrite",
        TxnMode::VersionChange => "versionchange",
    }
}

/// Pending-open trace: a wall-clock `idb.open` span.
///
/// Created at the JS `open()` call, closed when the open succeeds or fails.
/// With the `tracing` feature disabled this is a zero-sized type and both
/// constructors/finishers compile away.
#[cfg(feature = "tracing")]
#[derive(Debug, Clone)]
pub(crate) struct OpenTrace {
    span: tracing::Span,
    started: std::time::Instant,
}

/// Pending-open trace placeholder when `tracing` is disabled.
#[cfg(not(feature = "tracing"))]
#[derive(Debug, Clone)]
pub(crate) struct OpenTrace;

/// Starts the `idb.open` trace for a database open request.
#[cfg(feature = "tracing")]
pub(crate) fn start_open_trace(database: &str, requested_version: u64) -> OpenTrace {
    let span = tracing::info_span!(
        "idb.open",
        database = database.to_owned(),
        requested_version = requested_version,
        version = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
        error = tracing::field::Empty,
    );
    OpenTrace {
        span,
        started: std::time::Instant::now(),
    }
}

/// Starts the `idb.open` trace placeholder when `tracing` is disabled.
#[cfg(not(feature = "tracing"))]
pub(crate) fn start_open_trace(_database: &str, _requested_version: u64) -> OpenTrace {
    OpenTrace
}

#[cfg(feature = "tracing")]
impl OpenTrace {
    /// Closes the trace on open success.
    pub(crate) fn finish_success(self, version: u64) {
        self.span.record("version", version);
        self.span
            .record("duration_ms", self.started.elapsed().as_millis() as u64);
        drop(self);
    }

    /// Closes the trace on open failure.
    pub(crate) fn finish_error(self, error: &str) {
        self.span.record("error", error);
        self.span
            .record("duration_ms", self.started.elapsed().as_millis() as u64);
        drop(self);
    }
}

#[cfg(not(feature = "tracing"))]
#[allow(clippy::unused_self)]
impl OpenTrace {
    /// Closes the trace placeholder on open success.
    pub(crate) fn finish_success(self, _version: u64) {}

    /// Closes the trace placeholder on open failure.
    pub(crate) fn finish_error(self, _error: &str) {}
}

/// Creates the `idb.txn` span for a starting backend transaction.
///
/// The span is stored on the transaction handle and closed at commit/abort
/// with `duration_ms`, `bytes_read` and `bytes_written` recorded.
#[cfg(feature = "tracing")]
pub(crate) fn txn_span(
    database: &str,
    mode: TxnMode,
    scope_len: usize,
    txn_seq: u64,
) -> tracing::Span {
    tracing::info_span!(
        "idb.txn",
        database = database.to_owned(),
        mode = txn_mode_name(mode),
        scope_len = scope_len,
        txn_seq = txn_seq,
        duration_ms = tracing::field::Empty,
        bytes_read = tracing::field::Empty,
        bytes_written = tracing::field::Empty,
    )
}

/// Creates the `idb.request` span entered around backend execution.
#[cfg(feature = "tracing")]
pub(crate) fn request_span(txn_seq: u64, request_seq: u64) -> tracing::Span {
    tracing::info_span!(
        "idb.request",
        txn_seq = txn_seq,
        request_seq = request_seq,
        duration_us = tracing::field::Empty,
        bytes_read = tracing::field::Empty,
        bytes_written = tracing::field::Empty,
        failed = tracing::field::Empty,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_idb_core::error::IdbError;
    use boa_idb_core::proto::TxnMode;
    use std::time::Duration;

    #[test]
    fn classify_error_routes_quota_and_corruption() {
        assert_eq!(
            classify_error(&IdbError::QuotaExceeded {
                needed: 10,
                available: 5
            }),
            Some(ErrorClass::Quota)
        );
        assert_eq!(
            classify_error(&IdbError::NotReadable("torn".to_string())),
            Some(ErrorClass::Corruption)
        );
        for other in [
            IdbError::Abort,
            IdbError::Constraint("c".to_string()),
            IdbError::Data("d".to_string()),
            IdbError::Unknown("u".to_string()),
        ] {
            assert_eq!(classify_error(&other), None);
        }
    }

    #[test]
    fn error_name_covers_all_variants() {
        let cases: Vec<(IdbError, &str)> = vec![
            (IdbError::Constraint("c".to_string()), "ConstraintError"),
            (IdbError::TransactionInactive, "TransactionInactiveError"),
            (IdbError::DataClone("c".to_string()), "DataCloneError"),
            (
                IdbError::InvalidAccess("a".to_string()),
                "InvalidAccessError",
            ),
            (IdbError::InvalidState("s".to_string()), "InvalidStateError"),
            (IdbError::NotFound("n".to_string()), "NotFoundError"),
            (IdbError::ReadOnly, "ReadOnlyError"),
            (IdbError::Abort, "AbortError"),
            (IdbError::Syntax("s".to_string()), "SyntaxError"),
            (IdbError::Data("d".to_string()), "DataError"),
            (IdbError::Version("v".to_string()), "VersionError"),
            (
                IdbError::QuotaExceeded {
                    needed: 1,
                    available: 0,
                },
                "QuotaExceededError",
            ),
            (IdbError::NotReadable("r".to_string()), "NotReadableError"),
            (IdbError::Unknown("u".to_string()), "UnknownError"),
        ];
        for (error, name) in cases {
            assert_eq!(error_name(&error), name);
        }
    }

    #[test]
    fn txn_mode_names_and_time_helpers() {
        assert_eq!(txn_mode_name(TxnMode::ReadOnly), "readonly");
        assert_eq!(txn_mode_name(TxnMode::ReadWrite), "readwrite");
        assert_eq!(txn_mode_name(TxnMode::VersionChange), "versionchange");
        assert_eq!(millis(Duration::from_millis(1500)), 1500);
        assert_eq!(micros(Duration::from_micros(42)), 42);
        assert_eq!(millis(Duration::MAX), u64::MAX);
    }

    #[test]
    fn histogram_and_state_defaults() {
        let _histo = LatencyHistogram::default();
        let _state = ObserverState::default();
    }
}
