//! M7-A observability tests: [`IdbObserver`] lifecycle, counters, bounded
//! histograms, privacy, and (under the `tracing` feature) span topology.
//!
//! The JS scenario opens a database (upgrade with one store), commits a
//! readwrite transaction (`put` + `get`), then aborts a second readwrite
//! transaction. All assertions run against the recorded events and
//! [`IdbStats`], so they hold without any tracing subscriber.

use std::sync::{Arc, Mutex};

use boa_engine::{Context, Source};
use boa_idb::extension::IndexedDbExtension;
use boa_idb::observer::{HISTOGRAM_BOUNDS_US, HISTOGRAM_BUCKETS, IdbEvent, IdbObserver};
use boa_idb::runtime::IdbRuntime;
use boa_idb_core::proto::StorageKey;
use boa_idb_memory::MemoryBackendFactory;

/// Distinctive markers: they must never appear in events or span fields.
const KEY_MARKER: &str = "MARKER_KEY_XYZ";
const VALUE_MARKER: &str = "MARKER_VALUE_XYZ";

/// Thread-safe recording observer for tests.
#[derive(Debug, Default)]
struct RecordingObserver {
    events: Mutex<Vec<IdbEvent>>,
}

impl RecordingObserver {
    /// Returns a snapshot of all events observed so far.
    fn snapshot(&self) -> Vec<IdbEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl IdbObserver for RecordingObserver {
    fn on_event(&self, event: &IdbEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.clone());
    }
}

/// Creates a context with a recording observer registered.
fn create_context(sink: &Arc<RecordingObserver>) -> Context {
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("observer-tests"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .observer(Arc::clone(sink) as Arc<dyn IdbObserver>)
        .build()
        .expect("build extension");
    extension
        .register(&mut context)
        .expect("register extension");
    context
}

/// Runs the lifecycle scenario: upgrade + commit + abort.
fn run_lifecycle(context: &mut Context) {
    context
        .eval(Source::from_bytes(&format!(
            r"
            globalThis.__out = {{ aborted: false }};
            let openReq = indexedDB.open('obsdb', 1);
            openReq.onupgradeneeded = () => {{
                openReq.result.createObjectStore('s');
            }};
            openReq.onsuccess = () => {{
                const db = openReq.result;
                const tx = db.transaction('s', 'readwrite');
                tx.objectStore('s').put({{ secret: '{VALUE_MARKER}' }}, '{KEY_MARKER}');
                tx.objectStore('s').get('{KEY_MARKER}');
                tx.oncomplete = () => {{
                    const tx2 = db.transaction('s', 'readwrite');
                    const putReq = tx2.objectStore('s').put({{ secret: 'temp' }}, 'temp-key');
                    // Abort after the put executed: the request completes,
                    // then the transaction aborts (Begun + Aborted pair).
                    putReq.onsuccess = () => {{ tx2.abort(); }};
                    tx2.onabort = () => {{ globalThis.__out.aborted = true; }};
                }};
            }};
            "
        )))
        .expect("scenario eval");
    context.run_jobs().expect("jobs should run");
    context.run_jobs().expect("jobs should run");
    context.run_jobs().expect("jobs should run");
}

/// Counts events matching a predicate.
fn count(events: &[IdbEvent], mut pred: impl FnMut(&IdbEvent) -> bool) -> usize {
    events.iter().filter(|e| pred(e)).count()
}

#[test]
fn observer_lifecycle_counters_and_bytes() {
    let sink = Arc::new(RecordingObserver::default());
    let mut context = create_context(&sink);
    run_lifecycle(&mut context);

    let events = sink.snapshot();
    // One open, one upgrade txn + two normal txns.
    assert_eq!(
        count(&events, |e| matches!(
            e,
            IdbEvent::DatabaseOpened {
                database,
                version: 1
            } if database == "obsdb"
        )),
        1,
        "exactly one DatabaseOpened(obsdb, 1): {events:?}"
    );
    assert_eq!(
        count(&events, |e| matches!(e, IdbEvent::TransactionBegun { .. })),
        3,
        "upgrade + commit + abort txns begun: {events:?}"
    );
    assert_eq!(
        count(&events, |e| matches!(
            e,
            IdbEvent::TransactionCommitted { .. }
        )),
        2,
        "upgrade + first txn committed: {events:?}"
    );
    assert_eq!(
        count(&events, |e| matches!(
            e,
            IdbEvent::TransactionAborted { .. }
        )),
        1,
        "second txn aborted: {events:?}"
    );
    // Exact byte accounting lives in a helper below (line budget).
    let stats = context
        .get_data::<IdbRuntime>()
        .expect("runtime registered")
        .stats();
    assert_eq!(stats.txns_begun, 3, "begun counter");
    assert_eq!(stats.txns_committed, 2, "committed counter");
    assert_eq!(stats.txns_aborted, 1, "aborted counter");
    assert_eq!(stats.requests_completed, 3, "completed counter");
    assert_eq!(stats.requests_failed, 0, "no request failed");
    assert_exact_bytes(&events, &stats);
    assert_eq!(stats.observers, 1, "one host observer");
    // Every completed request fed the histogram exactly once.
    let request_samples: u64 = stats.request_latency.iter().sum();
    assert_eq!(
        request_samples,
        stats.requests_completed + stats.requests_failed,
        "histogram total matches completed requests"
    );
    let txn_samples: u64 = stats.txn_latency.iter().sum();
    assert_eq!(
        txn_samples, stats.txns_committed,
        "txn histogram matches committed txns"
    );
}

/// Exact byte-accounting assertions for the lifecycle scenario.
///
/// Executed requests in pump order: upgrade enqueues none; put + get run in
/// the committed txn, then the aborted txn's put executes before the abort.
/// Single-counting rule: global totals equal the request sum exactly —
/// neither doubled by `TransactionCommitted` nor dropped on abort — while
/// per-transaction totals still ride along for observers.
fn assert_exact_bytes(events: &[IdbEvent], stats: &boa_idb::observer::IdbStats) {
    let completed: Vec<&IdbEvent> = events
        .iter()
        .filter(|e| matches!(e, IdbEvent::RequestCompleted { .. }))
        .collect();
    assert_eq!(
        completed.len(),
        3,
        "put + get + aborted-txn put completed: {events:?}"
    );
    let request_bytes = |event: &IdbEvent| match event {
        IdbEvent::RequestCompleted {
            bytes_read,
            bytes_written,
            failed,
            ..
        } => {
            assert!(!failed, "scenario requests all succeed: {event:?}");
            (*bytes_read, *bytes_written)
        }
        _ => unreachable!("filtered to RequestCompleted"),
    };
    // put writes exactly the SCF payload and reads nothing ...
    let (put_r, put_w) = request_bytes(completed[0]);
    assert_eq!(put_r, 0, "put reads no payload: {events:?}");
    assert!(put_w > 0, "put writes its SCF payload: {events:?}");
    // ... get reads back the identical bytes and writes nothing ...
    let (get_r, get_w) = request_bytes(completed[1]);
    assert_eq!(get_w, 0, "get writes no payload: {events:?}");
    assert_eq!(
        get_r, put_w,
        "get returns the exact bytes the put stored: {events:?}"
    );
    // ... and the aborted txn's put is an ordinary executed request.
    let (abort_r, abort_w) = request_bytes(completed[2]);
    assert_eq!(abort_r, 0, "aborted put reads no payload: {events:?}");
    assert!(abort_w > 0, "aborted put writes its payload: {events:?}");
    assert_ne!(
        abort_w, put_w,
        "distinct values have distinct payload sizes: {events:?}"
    );
    assert_eq!(
        stats.bytes_written,
        put_w + abort_w,
        "written bytes counted exactly once (aborted write included): {events:?}"
    );
    assert_eq!(
        stats.bytes_read, get_r,
        "read bytes counted exactly once: {events:?}"
    );
    assert_eq!(
        count(events, |e| matches!(
            e,
            IdbEvent::TransactionCommitted {
                bytes_read: read,
                bytes_written: written,
                ..
            } if *read == put_w && *written == put_w
        )),
        1,
        "committed txn carries its per-transaction totals: {events:?}"
    );
}

#[test]
fn observer_privacy_no_payload_leak() {
    let sink = Arc::new(RecordingObserver::default());
    let mut context = create_context(&sink);
    run_lifecycle(&mut context);

    let events = sink.snapshot();
    let dump = format!("{events:?}");
    assert!(
        !dump.contains(KEY_MARKER),
        "key bytes must never reach observers"
    );
    assert!(
        !dump.contains(VALUE_MARKER),
        "value bytes must never reach observers"
    );
    // Database names are explicitly allowed attributes.
    assert!(dump.contains("obsdb"), "database name is recorded");
}

#[test]
fn histogram_is_bounded_and_indexed() {
    use boa_idb::observer::LatencyHistogram;
    use std::time::Duration;

    let mut hist = LatencyHistogram::new();
    assert_eq!(hist.buckets().len(), HISTOGRAM_BUCKETS);
    assert_eq!(HISTOGRAM_BOUNDS_US.len(), HISTOGRAM_BUCKETS);
    // One sample per bucket plus overflow and zero.
    hist.record(Duration::ZERO);
    hist.record(Duration::from_micros(1));
    hist.record(Duration::from_millis(5));
    hist.record(Duration::from_secs(60));
    hist.record(Duration::from_secs(3600 * 24 * 365 * 100));
    assert_eq!(hist.total(), 5);
    assert_eq!(hist.buckets().len(), HISTOGRAM_BUCKETS);
    // Overflow bucket is the last one and caught the century.
    assert!(hist.buckets()[HISTOGRAM_BUCKETS - 1] >= 1);
    // Bucket bounds are sorted (binary-searchable by construction).
    let mut sorted = HISTOGRAM_BOUNDS_US.to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, HISTOGRAM_BOUNDS_US.to_vec());
}

/// Minimal capturing `tracing` subscriber (no extra dependencies).
///
/// The subscriber is installed as the **process-global** default, shared by
/// every test thread. That is deliberate: `tracing` caches callsite interest
/// globally, so parallel tests with different thread-local dispatchers would
/// nondeterministically disable each other's spans. One shared enabled
/// dispatcher keeps capture deterministic; tests isolate their spans by
/// owning thread id (each test runs wholly on one thread).
#[cfg(feature = "tracing")]
mod capture {
    use std::fmt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread::ThreadId;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::Subscriber;
    use tracing::{Event, Metadata};

    /// One captured span with its fields merged across `new_span`/`record`.
    #[derive(Debug, Clone)]
    pub struct SpanRec {
        /// Owning test thread (isolation key).
        pub thread: ThreadId,
        /// Span name (`idb.open`, `idb.txn`, `idb.request`).
        pub name: String,
        /// `(field, debug-rendered value)` pairs.
        pub fields: Vec<(String, String)>,
    }

    struct FieldCap {
        fields: Vec<(String, String)>,
    }

    impl Visit for FieldCap {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.fields
                .push((field.name().to_owned(), format!("{value:?}")));
        }
    }

    #[derive(Debug, Default)]
    struct Inner {
        spans: Mutex<Vec<(u64, SpanRec)>>,
        next: AtomicU64,
    }

    #[derive(Debug)]
    struct GlobalCapture {
        inner: Arc<Inner>,
    }

    impl Subscriber for GlobalCapture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &Attributes<'_>) -> Id {
            let mut cap = FieldCap { fields: Vec::new() };
            span.record(&mut cap);
            let id = self.inner.next.fetch_add(1, Ordering::Relaxed) + 1;
            self.inner
                .spans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    id,
                    SpanRec {
                        thread: std::thread::current().id(),
                        name: span.metadata().name().to_owned(),
                        fields: cap.fields,
                    },
                ));
            Id::from_u64(id)
        }

        fn record(&self, id: &Id, values: &Record<'_>) {
            let mut cap = FieldCap { fields: Vec::new() };
            values.record(&mut cap);
            if let Some((_, rec)) = self
                .inner
                .spans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter_mut()
                .find(|(sid, _)| *sid == id.into_u64())
            {
                rec.fields.extend(cap.fields);
            }
        }

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, _event: &Event<'_>) {}

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    static CAPTURE: OnceLock<Arc<Inner>> = OnceLock::new();

    /// Installs the shared global subscriber (idempotent) and rebuilds the
    /// callsite interest cache so spans are enabled deterministically.
    pub fn install_global() {
        CAPTURE.get_or_init(|| {
            let inner = Arc::new(Inner::default());
            tracing::subscriber::set_global_default(GlobalCapture {
                inner: Arc::clone(&inner),
            })
            .expect("global tracing subscriber installs once");
            inner
        });
        tracing::callsite::rebuild_interest_cache();
    }

    /// Returns spans created on the given thread.
    pub fn spans_for(thread: ThreadId) -> Vec<SpanRec> {
        CAPTURE
            .get()
            .map(|inner| {
                inner
                    .spans
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .filter(|(_, rec)| rec.thread == thread)
                    .map(|(_, rec)| rec.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Concatenates every field value captured on the given thread.
    pub fn field_dump_for(thread: ThreadId) -> String {
        spans_for(thread)
            .iter()
            .flat_map(|rec| rec.fields.iter().map(|(_, v)| v.clone()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Finds spans by name on the given thread.
    pub fn by_name(thread: ThreadId, name: &str) -> Vec<SpanRec> {
        spans_for(thread)
            .into_iter()
            .filter(|rec| rec.name == name)
            .collect()
    }
}

/// Returns the debug-rendered value of a span field, if present.
#[cfg(feature = "tracing")]
fn span_field(span: &capture::SpanRec, name: &str) -> Option<String> {
    span.fields
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

/// Span names, required fields and payload privacy (tracing feature only).
#[cfg(feature = "tracing")]
#[test]
fn tracing_spans_names_fields_and_privacy() {
    capture::install_global();
    let me = std::thread::current().id();

    let sink = Arc::new(RecordingObserver::default());
    let mut context = create_context(&sink);
    run_lifecycle(&mut context);

    // `idb.open`: exactly one open call, closed with version + duration.
    let opens = capture::by_name(me, "idb.open");
    assert_eq!(opens.len(), 1, "one idb.open span: {opens:?}");
    assert_eq!(
        span_field(&opens[0], "database").as_deref(),
        Some("\"obsdb\"")
    );
    assert_eq!(span_field(&opens[0], "version").as_deref(), Some("1"));
    assert!(
        span_field(&opens[0], "duration_ms").is_some(),
        "open duration recorded: {opens:?}"
    );

    // `idb.txn`: upgrade + two normal transactions, all closed.
    let txns = capture::by_name(me, "idb.txn");
    assert_eq!(txns.len(), 3, "three idb.txn spans: {txns:?}");
    for span in &txns {
        for field in [
            "database",
            "mode",
            "scope_len",
            "txn_seq",
            "duration_ms",
            "bytes_read",
            "bytes_written",
        ] {
            assert!(
                span_field(span, field).is_some(),
                "txn span has {field}: {span:?}"
            );
        }
    }
    let modes: Vec<String> = txns.iter().filter_map(|s| span_field(s, "mode")).collect();
    assert!(
        modes.contains(&"\"versionchange\"".to_owned()),
        "upgrade txn spanned: {modes:?}"
    );
    assert_eq!(
        modes.iter().filter(|m| *m == "\"readwrite\"").count(),
        2,
        "two readwrite txns spanned: {modes:?}"
    );

    // `idb.request`: every executed request, closed with timing + outcome.
    let requests = capture::by_name(me, "idb.request");
    assert!(
        requests.len() >= 3,
        "put + get + aborted-txn put spanned: {requests:?}"
    );
    for span in &requests {
        for field in [
            "txn_seq",
            "request_seq",
            "duration_us",
            "bytes_read",
            "bytes_written",
            "failed",
        ] {
            assert!(
                span_field(span, field).is_some(),
                "request span has {field}: {span:?}"
            );
        }
    }

    // Privacy: payload markers must not appear in any span field.
    let dump = capture::field_dump_for(me);
    assert!(
        !dump.contains(KEY_MARKER),
        "key bytes must never reach spans"
    );
    assert!(
        !dump.contains(VALUE_MARKER),
        "value bytes must never reach spans"
    );
}
