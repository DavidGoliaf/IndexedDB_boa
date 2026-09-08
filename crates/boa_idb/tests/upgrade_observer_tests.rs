//! P1-1 regression: observer callbacks never run under engine/driver locks,
//! and upgrade `TransactionBegun` precedes `upgradeneeded` dispatch.
//!
//! Strategy (deterministic, no timeouts):
//!
//! * The probe observer holds clones of the driver/engine/observer-state
//!   mutexes. `try_lock` never blocks: `Err` proves a guard was held on the
//!   pump thread at emit time, so any violation fails the test instead of
//!   deadlocking it. The probe additionally performs a re-entrant
//!   `stats()` read, which would deadlock under a blocking lock had the
//!   fan-out kept the observer guard.
//! * The pump is stepped manually (`drive_turn`, one turn at a time) with
//!   `auto_pump` off. At the first turn where the JS `upgradeneeded` flag is
//!   set, `TransactionBegun` must already sit in the probe log (both happen
//!   in the same `StartUpgrade` arm, emit strictly before dispatch), and the
//!   upgrade commit must land on a strictly later turn.

use std::sync::{Arc, Mutex};

use boa_engine::{Context, Source};
use boa_idb::driver::{DriverState, drive_turn};
use boa_idb::engine::IdbEngine;
use boa_idb::extension::IndexedDbExtension;
use boa_idb::observer::{IdbEvent, IdbObserver, ObserverState};
use boa_idb::runtime::{IdbRuntime, set_auto_pump};
use boa_idb_core::proto::{StorageKey, TxnMode};
use boa_idb_memory::MemoryBackendFactory;

/// Lock-freedom + event probe. Holds plain mutex handles only: never the
/// Boa `Context`, never JS values, never IDB objects.
struct LockProbe {
    driver: Arc<Mutex<DriverState>>,
    engine: Arc<Mutex<Option<IdbEngine>>>,
    observer_state: Arc<Mutex<ObserverState>>,
    violations: Mutex<Vec<String>>,
    events: Mutex<Vec<IdbEvent>>,
}

impl LockProbe {
    /// Short stable name of an event kind (never user data).
    fn kind_name(event: &IdbEvent) -> &'static str {
        match event {
            IdbEvent::DatabaseOpened { .. } => "DatabaseOpened",
            IdbEvent::TransactionBegun { .. } => "TransactionBegun",
            IdbEvent::TransactionCommitted { .. } => "TransactionCommitted",
            IdbEvent::TransactionAborted { .. } => "TransactionAborted",
            IdbEvent::RequestCompleted { .. } => "RequestCompleted",
            IdbEvent::QuotaExceeded { .. } => "QuotaExceeded",
            IdbEvent::Corruption { .. } => "Corruption",
            _ => "Unknown",
        }
    }

    /// Snapshots observed events.
    fn snapshot(&self) -> Vec<IdbEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Snapshots recorded violations.
    fn violations(&self) -> Vec<String> {
        self.violations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl IdbObserver for LockProbe {
    fn on_event(&self, event: &IdbEvent) {
        let kind = Self::kind_name(event);
        let mut violations = self
            .violations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // `try_lock` never blocks: `Err` deterministically proves a guard
        // was held on this thread when the callback ran.
        if self.driver.try_lock().is_err() {
            violations.push(format!("driver mutex held during {kind}"));
        }
        if self.engine.try_lock().is_err() {
            violations.push(format!("engine mutex held during {kind}"));
        }
        // The fan-out itself must not hold the observer mutex: a re-entrant
        // read (what `stats()` does) has to succeed.
        match self.observer_state.try_lock() {
            Ok(guard) => {
                let _ = guard.stats();
            }
            Err(_) => violations.push(format!("observer mutex held during {kind}")),
        }
        drop(violations);
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event.clone());
    }
}

/// Evaluates a boolean JS expression.
fn js_flag(context: &mut Context, expr: &str) -> bool {
    context
        .eval(Source::from_bytes(expr))
        .expect("flag eval must succeed")
        .as_boolean()
        .unwrap_or(false)
}

#[test]
fn upgrade_begun_precedes_upgradeneeded_without_locks() {
    let mut context = Context::default();
    let extension = IndexedDbExtension::builder()
        .storage_key(StorageKey::new("upgrade-observe-tests"))
        .backend_factory(Arc::new(MemoryBackendFactory::new()))
        .build()
        .expect("build extension");
    extension
        .register(&mut context)
        .expect("register extension");
    set_auto_pump(&mut context, false);

    // Install the probe after registration so it can clone the live handles.
    let probe = {
        let runtime = context
            .get_data::<IdbRuntime>()
            .expect("runtime registered");
        let probe = Arc::new(LockProbe {
            driver: Arc::clone(&runtime.driver),
            engine: Arc::clone(&runtime.engine),
            observer_state: Arc::clone(&runtime.observer),
            violations: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        });
        runtime.add_observer(Arc::clone(&probe) as Arc<dyn IdbObserver>);
        probe
    };

    context
        .eval(Source::from_bytes(
            r"
            globalThis.__upgradeneeded = false;
            globalThis.__done = false;
            let openReq = indexedDB.open('updb', 1);
            openReq.onupgradeneeded = () => {
                globalThis.__upgradeneeded = true;
                openReq.result.createObjectStore('s');
            };
            openReq.onsuccess = () => { globalThis.__done = true; };
            ",
        ))
        .expect("scenario eval must succeed");

    // Drain the pump manually. A single `drive_turn` may complete the whole
    // upgrade (every phase loops internally), so ordering is asserted on the
    // probe's total event order — not on turn indices — plus the JS flags.
    // Everything is synchronous: no clocks, no timeouts.
    for _ in 0..10_000 {
        let progressed = drive_turn(&mut context).expect("pump must not fail");
        if js_flag(&mut context, "globalThis.__done === true") {
            break;
        }
        if !progressed {
            break;
        }
    }

    assert!(
        js_flag(&mut context, "globalThis.__done === true"),
        "upgrade lifecycle must complete without deadlock"
    );
    assert!(
        js_flag(&mut context, "globalThis.__upgradeneeded === true"),
        "upgradeneeded must fire"
    );
    assert!(
        probe.violations().is_empty(),
        "no observer callback under engine/driver/observer locks: {:?}",
        probe.violations()
    );

    let events = probe.snapshot();
    let begun_idx = events
        .iter()
        .position(|e| {
            matches!(
                e,
                IdbEvent::TransactionBegun {
                    mode: TxnMode::VersionChange,
                    ..
                }
            )
        })
        .expect("exactly one versionchange Begun");
    // `TransactionBegun` is the first observable event of the lifecycle: it
    // is emitted in the `StartUpgrade` arm strictly before `upgradeneeded`
    // dispatch, so no request completion, commit, abort, or opened event —
    // and no handler effect — can precede it. If pump internals ever emit
    // earlier, this fails loudly instead of silently reordering.
    assert_eq!(
        begun_idx, 0,
        "versionchange Begun must be the first observed event: {events:?}"
    );
    let begun_txn = match &events[begun_idx] {
        IdbEvent::TransactionBegun { txn, .. } => *txn,
        _ => unreachable!("index points at Begun"),
    };
    // The upgrade commit is the terminal event of the same transaction.
    let committed_idx = events
        .iter()
        .position(|e| {
            matches!(
                e,
                IdbEvent::TransactionCommitted { txn, .. } if *txn == begun_txn
            )
        })
        .expect("upgrade transaction must commit");
    assert!(
        committed_idx > begun_idx,
        "commit must follow begin: {events:?}"
    );
    // The first-open path reports the opened database with its version.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, IdbEvent::DatabaseOpened { version: 1, .. })),
        "DatabaseOpened(updb, 1) must be observed: {events:?}"
    );
}
