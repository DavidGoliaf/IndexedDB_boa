//! Runtime state for IndexedDB.
//!
//! [`IdbRuntime`] lives in the Boa [`Context`] host data. It owns the
//! [`IdbEngine`] and the plain [`DriverState`][crate::driver::DriverState].

use crate::engine::IdbEngine;
use boa_engine::job::{GenericJob, Job};
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue};
use boa_gc::{Finalize, GcRefCell, Trace};
use boa_idb_core::backend::traits::BackendFactory;
use boa_idb_core::error::IdbError;
use boa_idb_core::proto::StorageKey;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// Locks a mutex, recovering from poisoning.
///
/// Poisoning only happens when a thread panics while holding the lock.
/// Library code never panics on JS-reachable paths, so a poisoned lock can
/// only come from an embedding bug; proceeding with the guarded state is
/// safer than panicking the host. This also keeps every lock site panic-free
/// (`deny` on `unwrap`/`expect` along JS paths).
pub fn lock_mutex<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runtime state for IndexedDB, stored in the Boa Context's HostDefined data.
#[derive(Trace, Finalize, boa_engine::JsData)]
pub struct IdbRuntime {
    /// Storage key for this context.
    #[unsafe_ignore_trace]
    pub storage_key: StorageKey,
    /// Backend factory for creating storage instances.
    #[unsafe_ignore_trace]
    pub backend_factory: Arc<dyn BackendFactory>,
    /// The engine, lazily initialized on first use.
    ///
    /// Lock discipline (single-threaded execution): this lock — like the
    /// driver lock — is NEVER held across JS event dispatch. The driver pump
    /// scopes every lock to pure state steps; dispatch runs lock-free, so
    /// reentrant API calls from event handlers cannot deadlock.
    #[unsafe_ignore_trace]
    pub engine: Arc<Mutex<Option<IdbEngine>>>,
    /// Traced registry of pending request objects, keyed by request id.
    pub request_objects: GcRefCell<BTreeMap<u64, JsObject>>,
    /// Traced registry of live cursor objects.
    pub cursor_objects: GcRefCell<BTreeMap<u64, JsObject>>,
    /// Traced registry of live transaction objects.
    pub txn_objects: GcRefCell<BTreeMap<u64, JsObject>>,
    /// Identity registry for `IDBKeyRange` objects.
    pub key_range_objects: GcRefCell<Vec<JsObject>>,
    /// GC-backed identity set for `IDBKeyRange` objects.
    pub key_range_set: GcRefCell<Option<JsObject>>,
    /// Plain (non-GC) driver state: pending opens, transactions, requests, cursors.
    #[unsafe_ignore_trace]
    pub driver: Arc<Mutex<crate::driver::DriverState>>,
    /// Whether API calls schedule pump jobs (`true` by default).
    ///
    /// Embeddings that drive the pump themselves (the WPT runner calls
    /// [`crate::driver::drive_turn`] directly for fair interleaving with
    /// promise jobs and virtual timers) set this to `false`: a scheduled
    /// pump job drains to quiescence inside [`Context::run_jobs`], which an
    /// endless-but-productive queue (e.g. a `keep_alive` spin) would never
    /// let return.
    #[unsafe_ignore_trace]
    pub auto_pump: Cell<bool>,
}

impl IdbRuntime {
    /// Creates a new `IdbRuntime`.
    ///
    /// The `Arc`s below are never shared across threads (Boa contexts are
    /// single-threaded); they only express shared ownership, hence the
    /// targeted pedantic allow.
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(storage_key: StorageKey, backend_factory: Arc<dyn BackendFactory>) -> Self {
        Self {
            storage_key,
            backend_factory,
            engine: Arc::new(Mutex::new(None)),
            request_objects: GcRefCell::default(),
            cursor_objects: GcRefCell::default(),
            txn_objects: GcRefCell::default(),
            key_range_objects: GcRefCell::default(),
            key_range_set: GcRefCell::default(),
            driver: Arc::new(Mutex::new(crate::driver::DriverState::default())),
            auto_pump: Cell::new(true),
        }
    }

    /// Initializes the engine if not already done.
    fn ensure_engine(&self) -> Result<(), IdbError> {
        let mut guard = lock_mutex(&self.engine);
        if guard.is_none() {
            *guard = Some(IdbEngine::new(
                self.backend_factory.clone(),
                &self.storage_key,
            )?);
        }
        Ok(())
    }
}

/// Executes a closure with mutable access to the engine.
///
/// Free function (not a method): API closures usually need `&mut Context`
/// as well, which cannot coexist with a borrow of runtime data held through
/// `Context::get_data`. Handles are cloned first so no borrow is retained.
pub fn with_engine<R>(context: &mut Context, f: impl FnOnce(&mut IdbEngine) -> R) -> JsResult<R> {
    let (handle, factory, key) = {
        let runtime = context
            .get_data::<IdbRuntime>()
            .ok_or_else(|| JsNativeError::error().with_message("IndexedDB not initialized"))?;
        (
            Arc::clone(&runtime.engine),
            runtime.backend_factory.clone(),
            runtime.storage_key.clone(),
        )
    };
    {
        let mut guard = lock_mutex(&handle);
        if guard.is_none() {
            let engine = IdbEngine::new(factory, &key)
                .map_err(|e| crate::dom::exception::throw_idb_error(&e, context))?;
            *guard = Some(engine);
        }
    }
    let mut guard = lock_mutex(&handle);
    match guard.as_mut() {
        Some(engine) => Ok(f(engine)),
        None => Err(JsNativeError::error()
            .with_message("IndexedDB engine is not initialized")
            .into()),
    }
}

/// Schedules a driver-pump job unless one is already scheduled.
///
/// API methods only enqueue work; actual execution (and event dispatch)
/// happens in the pump job, i.e. as a separate Boa task after the current
/// one — matching the asynchronous IndexedDB model.
///
/// No-op when [`IdbRuntime::auto_pump`] is disabled: the embedding drives
/// [`crate::driver::drive_turn`] itself (see [`pump_all`]).
pub fn schedule_pump(context: &mut Context) {
    let Some(runtime) = context.get_data::<IdbRuntime>() else {
        return;
    };
    if !runtime.auto_pump.get() {
        return;
    }
    let already = {
        let mut d = lock_mutex(&runtime.driver);
        if d.pump_scheduled {
            true
        } else {
            d.pump_scheduled = true;
            false
        }
    };
    if already {
        return;
    }
    let realm = context.realm().clone();
    context.enqueue_job(Job::GenericJob(GenericJob::new(
        |context| {
            // Drain while turns make progress; follow-up work enqueued by
            // event handlers is picked up by the loop. Remaining-but-stalled
            // work (e.g. blocked opens) waits for a future pump scheduled by
            // the unblocking event. No JS runs between the loop exit and the
            // flag reset below (single-threaded), so no work can be stranded.
            while crate::driver::drive_turn(context)? {}
            if let Some(runtime) = context.get_data::<IdbRuntime>() {
                lock_mutex(&runtime.driver).pump_scheduled = false;
            }
            Ok(JsValue::undefined())
        },
        realm,
    )));
}

/// Enables or disables automatic pump-job scheduling for a context.
///
/// Disabled by embeddings (like the WPT runner) that call
/// [`crate::driver::drive_turn`] directly.
pub fn set_auto_pump(context: &mut Context, auto_pump: bool) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        runtime.auto_pump.set(auto_pump);
    }
}

/// Pumps the driver synchronously until no work remains.
///
/// Test and embedding helper for environments without a Boa job loop.
/// Returns the number of turns executed.
pub fn pump_all(context: &mut Context) -> JsResult<usize> {
    let mut turns = 0;
    // Clear a stale scheduled flag: explicit pumping takes over the job's role.
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        lock_mutex(&runtime.driver).pump_scheduled = false;
    }
    while crate::driver::drive_turn(context)? {
        turns += 1;
    }
    Ok(turns)
}

/// Records a request object in the traced registry.
pub fn register_request(context: &mut Context, request_id: u64, object: JsObject) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        runtime
            .request_objects
            .borrow_mut()
            .insert(request_id, object);
    }
}

/// Looks up a request object by id.
pub fn request_object(context: &Context, request_id: u64) -> Option<JsObject> {
    context
        .get_data::<IdbRuntime>()
        .and_then(|r| r.request_objects.borrow().get(&request_id).cloned())
}

/// Records a cursor object in the traced registry.
pub fn register_cursor(context: &mut Context, cursor_id: u64, object: JsObject) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        runtime
            .cursor_objects
            .borrow_mut()
            .insert(cursor_id, object);
    }
}

/// Looks up a cursor object by id.
pub fn cursor_object(context: &Context, cursor_id: u64) -> Option<JsObject> {
    context
        .get_data::<IdbRuntime>()
        .and_then(|r| r.cursor_objects.borrow().get(&cursor_id).cloned())
}

/// Records a transaction object in the traced registry.
pub fn register_txn(context: &mut Context, txn_id: u64, object: JsObject) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        runtime.txn_objects.borrow_mut().insert(txn_id, object);
    }
}

/// Looks up a transaction object by id.
pub fn txn_object(context: &Context, txn_id: u64) -> Option<JsObject> {
    context
        .get_data::<IdbRuntime>()
        .and_then(|r| r.txn_objects.borrow().get(&txn_id).cloned())
}

/// Removes a transaction object from the traced registry.
pub fn unregister_txn(context: &Context, txn_id: u64) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        runtime.txn_objects.borrow_mut().remove(&txn_id);
    }
}

/// Hook called at the end of each task: deactivates unfinished transactions
/// (AD-8, §2.7.1 "cleanup Indexed Database transactions").
///
/// Already-enqueued requests keep their place in the driver queues and still
/// execute; only *new* requests on these transactions will throw
/// `TransactionInactiveError`.
pub fn end_of_task(context: &mut Context) {
    if let Some(runtime) = context.get_data::<IdbRuntime>() {
        let mut d = lock_mutex(&runtime.driver);
        for handle in d.txns.values_mut() {
            handle.active = false;
        }
        let ids: Vec<u64> = d
            .txns
            .iter()
            .filter(|(_, t)| !t.finished)
            .map(|(id, _)| *id)
            .collect();
        drop(d);
        for id in ids {
            if let Some(obj) = txn_object(context, id) {
                if let Some(mut data) =
                    obj.downcast_mut::<crate::api::transaction::IdBTransaction>()
                {
                    data.active = false;
                }
            }
        }
    }
}
