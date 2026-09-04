//! Test environment construction: a fresh [`Context`] per test with browser
//! polyfills (`self`, `location`, timers, `structuredClone`, `EventTarget`,
//! `console`, `performance`) and an isolated temp storage directory.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::sync::Arc;

use boa_engine::native_function::NativeFunction;
use boa_engine::object::builtins::JsArrayBuffer;
use boa_engine::{Context, JsNativeError, JsObject, JsResult, JsValue, js_string};
use boa_idb::extension::IndexedDbExtension;
use boa_idb_core::proto::StorageKey;
use boa_idb_memory::MemoryBackendFactory;
use boa_idb_sqlite::SqliteBackendFactory;
use parking_lot::Mutex;

use crate::harness::{HarnessState, install_wpt_reporter};

/// Storage backend selected on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// In-memory backend (`boa_idb_memory`).
    Memory,
    /// `SQLite` backend under an isolated temporary directory.
    Sqlite,
}

impl Backend {
    /// Parses a backend from a CLI string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "memory" => Some(Self::Memory),
            "sqlite" => Some(Self::Sqlite),
            _ => None,
        }
    }

    /// CLI display name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Sqlite => "sqlite",
        }
    }
}

/// A scheduled macro-task.
struct TimerEntry {
    /// Unique timer id returned to JS.
    id: u64,
    /// Virtual-clock due time in milliseconds.
    due: f64,
    /// Callback to invoke when the timer fires.
    callback: JsValue,
}

/// Macro-task registry stored in the Context `HostDefined` data.
///
/// The runner drives the virtual clock, so `setTimeout` is deterministic and
/// does not depend on wall-clock time.
pub struct Timers {
    next_id: Cell<u64>,
    now: Cell<f64>,
    pending: RefCell<Vec<TimerEntry>>,
}

impl Default for Timers {
    fn default() -> Self {
        Self {
            // Start above the sentinel id used by testharness cleanup. A
            // stale clearTimeout(0) must not cancel the first real timer.
            next_id: Cell::new(1),
            now: Cell::new(0.0),
            pending: RefCell::new(Vec::new()),
        }
    }
}

impl Timers {
    fn schedule(&self, callback: JsValue, delay: f64) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let due = self.now.get() + delay.max(0.0);
        self.pending
            .borrow_mut()
            .push(TimerEntry { id, due, callback });
        id
    }

    fn cancel(&self, id: u64) {
        self.pending.borrow_mut().retain(|e| e.id != id);
    }

    /// Returns the earliest pending due time (virtual clock), if any.
    fn next_due(&self) -> Option<f64> {
        self.pending.borrow().iter().map(|e| e.due).reduce(f64::min)
    }

    /// Pops all timers due at or before `now`; returns their callbacks.
    fn pop_due(&self) -> Vec<JsValue> {
        let now = self.now.get();
        let mut pending = self.pending.borrow_mut();
        let (ready, keep): (Vec<_>, Vec<_>) = pending.drain(..).partition(|e| e.due <= now);
        *pending = keep;
        ready.into_iter().map(|e| e.callback).collect()
    }
}

/// Installs `setTimeout`/`clearTimeout` backed by the virtual-clock [`Timers`].
fn install_timers(context: &mut Context) -> JsResult<()> {
    context.insert_data(Timers::default());

    let set_timeout = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let callback = args.first().cloned().unwrap_or(JsValue::undefined());
        let delay = args
            .get(1)
            .and_then(|v| v.to_number(ctx).ok())
            .unwrap_or(0.0);
        let id = match ctx.get_data::<Timers>() {
            Some(timers) => timers.schedule(callback, delay),
            None => 0,
        };
        Ok(JsValue::from(id))
    });

    let clear_timeout = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let id = args
            .first()
            .and_then(|v| v.to_number(ctx).ok())
            .unwrap_or(0.0) as u64;
        if let Some(timers) = ctx.get_data::<Timers>() {
            timers.cancel(id);
        }
        Ok(JsValue::undefined())
    });

    let realm = context.realm().clone();
    context.global_object().set(
        js_string!("setTimeout"),
        JsValue::from(set_timeout.to_js_function(&realm)),
        false,
        context,
    )?;
    context.global_object().set(
        js_string!("clearTimeout"),
        JsValue::from(clear_timeout.to_js_function(&realm)),
        false,
        context,
    )?;
    Ok(())
}

/// Fires all timers that are due at current virtual time (`due <= now`).
///
/// Returns `true` if at least one callback ran.
pub fn fire_due_timers(context: &mut Context) -> JsResult<bool> {
    let ready = {
        let Some(timers) = context.get_data::<Timers>() else {
            return Ok(false);
        };
        timers.pop_due()
    };

    let mut fired = false;
    for callback in ready {
        fired = true;
        if let Some(callable) = callback.as_callable() {
            callable.call(&JsValue::undefined(), &[], context)?;
        }
    }
    Ok(fired)
}

/// Advances the virtual clock to the next scheduled timer, if any.
pub fn advance_to_next_timer(context: &Context) -> bool {
    let Some(timers) = context.get_data::<Timers>() else {
        return false;
    };
    if let Some(due) = timers.next_due() {
        timers.now.set(due);
        true
    } else {
        false
    }
}

/// Returns `true` when the context still has scheduled (underexecuted) timers.
pub fn timers_pending(context: &Context) -> bool {
    context
        .get_data::<Timers>()
        .is_some_and(|t| !t.pending.borrow().is_empty())
}

/// Installs the browser-surface polyfills required by `testharness.js` and by
/// the WPT tests (pure-JS portion).
#[allow(clippy::too_many_lines)]
fn install_js_polyfills(context: &mut Context) -> JsResult<()> {
    context.eval(boa_engine::Source::from_bytes(
        r#"
        globalThis.self = globalThis;
        globalThis.window = globalThis;
        globalThis.location = {
            href: "http://localhost/IndexedDB/",
            origin: "http://localhost",
            protocol: "http:",
            host: "localhost",
            hostname: "localhost",
            port: "",
            pathname: "/IndexedDB/",
            search: "",
            hash: ""
        };

        (function() {
            if (typeof globalThis.EventTarget === 'function') { return; }
            var EventTarget = function() { this.__wpt_listeners = {}; };
            EventTarget.prototype.addEventListener = function(type, cb) {
                if (!cb) { return; }
                if (!this.__wpt_listeners[type]) { this.__wpt_listeners[type] = []; }
                this.__wpt_listeners[type].push(cb);
            };
            EventTarget.prototype.removeEventListener = function(type, cb) {
                var list = this.__wpt_listeners[type] || [];
                this.__wpt_listeners[type] = list.filter(function(f) { return f !== cb; });
            };
            EventTarget.prototype.dispatchEvent = function(ev) {
                if (!ev || typeof ev.type !== 'string') { throw new TypeError('Invalid event'); }
                try { ev.target = this; } catch (e) {}
                var list = (this.__wpt_listeners[ev.type] || []).slice();
                for (var i = 0; i < list.length; i++) {
                    list[i].call(this, ev);
                }
                return true;
            };
            globalThis.EventTarget = EventTarget;
        })();

        if (typeof globalThis.GLOBAL === 'undefined') {
            globalThis.GLOBAL = { isShadowRealm: function() { return false; } };
        }

        // --- Minimal platform-object stubs for structured-clone WPT --------
        // These exist so test files referencing the globals evaluate; only
        // plain-data behavior is implemented. Exotic clone semantics (real
        // Blob bytes, geometry methods) are out of scope and covered by
        // expectations, not by these stubs.
        (function() {
            if (typeof globalThis.DOMPointReadOnly === 'undefined') {
                globalThis.DOMPointReadOnly = class DOMPointReadOnly {
                    constructor(x = 0, y = 0, z = 0, w = 1) {
                        this.x = +x; this.y = +y; this.z = +z; this.w = +w;
                    }
                    static fromPoint(p) {
                        return new DOMPointReadOnly(p.x, p.y, p.z, p.w);
                    }
                    toJSON() { return { x: this.x, y: this.y, z: this.z, w: this.w }; }
                };
            }
            if (typeof globalThis.DOMPoint === 'undefined') {
                globalThis.DOMPoint = class DOMPoint extends globalThis.DOMPointReadOnly {};
            }
            if (typeof globalThis.DOMRectReadOnly === 'undefined') {
                globalThis.DOMRectReadOnly = class DOMRectReadOnly {
                    constructor(x = 0, y = 0, width = 0, height = 0) {
                        this.x = +x; this.y = +y;
                        this.width = +width; this.height = +height;
                    }
                    get top() { return this.y; }
                    get left() { return this.x; }
                    get right() { return this.x + this.width; }
                    get bottom() { return this.y + this.height; }
                    static fromRect(r) {
                        return new DOMRectReadOnly(r.x, r.y, r.width, r.height);
                    }
                    toJSON() {
                        return { x: this.x, y: this.y, width: this.width, height: this.height };
                    }
                };
            }
            if (typeof globalThis.DOMRect === 'undefined') {
                globalThis.DOMRect = class DOMRect extends globalThis.DOMRectReadOnly {};
            }
            if (typeof globalThis.DOMMatrixReadOnly === 'undefined') {
                globalThis.DOMMatrixReadOnly = class DOMMatrixReadOnly {
                    constructor(init) {
                        this.m11 = 1; this.m12 = 0; this.m13 = 0; this.m14 = 0;
                        this.m21 = 0; this.m22 = 1; this.m23 = 0; this.m24 = 0;
                        this.m31 = 0; this.m32 = 0; this.m33 = 1; this.m34 = 0;
                        this.m41 = 0; this.m42 = 0; this.m43 = 0; this.m44 = 1;
                        this.is2D = true; this.isIdentity = true;
                        if (typeof init === 'string') { this.__css = String(init); }
                        else if (Array.isArray(init)) {
                            const names = ['m11','m12','m13','m14','m21','m22','m23','m24',
                                           'm31','m32','m33','m34','m41','m42','m43','m44'];
                            if (init.length === 16) {
                                for (let i = 0; i < 16; i++) { this[names[i]] = +init[i]; }
                                this.isIdentity = false;
                            } else if (init.length === 6) {
                                this.m11 = +init[0]; this.m12 = +init[1];
                                this.m21 = +init[2]; this.m22 = +init[3];
                                this.m41 = +init[4]; this.m42 = +init[5];
                                this.isIdentity = false;
                            }
                        }
                    }
                    static fromMatrix(m) { return new DOMMatrixReadOnly(m); }
                    toFloat64Array() {
                        return new Float64Array([this.m11, this.m12, this.m13, this.m14,
                            this.m21, this.m22, this.m23, this.m24,
                            this.m31, this.m32, this.m33, this.m34,
                            this.m41, this.m42, this.m43, this.m44]);
                    }
                };
            }
            if (typeof globalThis.DOMMatrix === 'undefined') {
                globalThis.DOMMatrix = class DOMMatrix extends globalThis.DOMMatrixReadOnly {};
            }
            if (typeof globalThis.ImageData === 'undefined') {
                globalThis.ImageData = class ImageData {
                    constructor(width, height) {
                        this.width = width | 0; this.height = height | 0;
                        this.data = new Uint8ClampedArray(this.width * this.height * 4);
                    }
                };
            }
            function __wpt_blob_size(part) {
                if (typeof part === 'string') { return part.length; }
                if (part instanceof ArrayBuffer) { return part.byteLength; }
                if (ArrayBuffer.isView(part)) { return part.byteLength; }
                if (part instanceof globalThis.Blob) { return part.size; }
                return 0;
            }
            if (typeof globalThis.Blob === 'undefined') {
                globalThis.Blob = class Blob {
                    constructor(parts = [], options = {}) {
                        this.__parts = Array.isArray(parts) ? parts.slice() : [parts];
                        this.type = String((options && options.type) || '').toLowerCase();
                    }
                    get size() {
                        return this.__parts.reduce((n, p) => n + __wpt_blob_size(p), 0);
                    }
                    slice(start = 0, end = this.size, type = '') {
                        return new globalThis.Blob([], { type });
                    }
                };
            }
            if (typeof globalThis.File === 'undefined') {
                globalThis.File = class File extends globalThis.Blob {
                    constructor(parts = [], name = '', options = {}) {
                        super(parts, options);
                        this.name = String(name);
                        this.lastModified = options && options.lastModified !== undefined
                            ? +options.lastModified : Date.now();
                    }
                };
            }
        })();
        "#,
    ))?;
    Ok(())
}

/// Installs a minimal `MessageChannel`/`MessagePort` pair.
///
/// Only `postMessage(message, transfer)` semantics needed by WPT are
/// implemented: every `ArrayBuffer` in the transfer list is detached via
/// `transfer()` (the source becomes neutered, as `createDetachedArrayBuffer`
/// asserts). Messages are not routed anywhere (no worker support).
fn install_message_channel(context: &mut Context) -> JsResult<()> {
    let detach_transfer = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        if let Some(list) = args.get(1)
            && let Some(list_obj) = list.as_object()
        {
            let mut index = 0u32;
            loop {
                let item = list_obj.get(index, ctx).unwrap_or(JsValue::undefined());
                if item.is_undefined() {
                    break;
                }
                if let Some(buf_obj) = item.as_object()
                    && let Ok(buffer) = JsArrayBuffer::from_object(buf_obj)
                {
                    buffer.detach(&JsValue::undefined())?;
                }
                index += 1;
                if index > 1024 {
                    break;
                }
            }
        }
        Ok(JsValue::undefined())
    });
    let realm = context.realm().clone();
    context.global_object().set(
        js_string!("__wpt_port_postMessage"),
        JsValue::from(detach_transfer.to_js_function(&realm)),
        false,
        context,
    )?;
    context.eval(boa_engine::Source::from_bytes(
        r"
        globalThis.MessageChannel = function() {
                function makePort() {
                    return {
                        postMessage: globalThis.__wpt_port_postMessage,
                        onmessage: null,
                        start: function() {},
                        close: function() {},
                        addEventListener: function() {},
                        removeEventListener: function() {},
                        dispatchEvent: function() { return true; }
                    };
                }
                this.port1 = makePort();
                this.port2 = makePort();
        };
        ",
    ))?;
    Ok(())
}

/// Installs a `structuredClone` polyfill backed by the `IndexedDB` structured
/// clone codec (SCF round-trip through `boa_idb`).
fn install_structured_clone(context: &mut Context) -> JsResult<()> {
    let structured_clone = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let value = args.first().cloned().unwrap_or(JsValue::undefined());
        let sc = boa_idb::convert::value::serialize_for_storage(&value, ctx)
            .map_err(|e| JsNativeError::error().with_message(format!("DataCloneError: {e}")))?;
        boa_idb::convert::value::deserialize_from_storage(&sc, ctx)
    });
    let realm = context.realm().clone();
    context.global_object().set(
        js_string!("structuredClone"),
        JsValue::from(structured_clone.to_js_function(&realm)),
        false,
        context,
    )?;
    Ok(())
}

/// Installs a minimal `console` that routes output into the harness state.
fn install_console(context: &mut Context, _state: &Arc<Mutex<HarnessState>>) -> JsResult<()> {
    let console_obj = JsObject::with_null_proto();
    let realm = context.realm().clone();
    for level in ["log", "warn", "error", "info", "debug"] {
        let f = NativeFunction::from_copy_closure(move |_this, args, ctx| {
            let text = args
                .iter()
                .map(|v| {
                    v.to_string(ctx)
                        .map_or_else(|_| "?".to_string(), |s| s.to_std_string_escaped())
                })
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(sink) = crate::harness::state_sink(ctx) {
                sink.lock().messages.push(format!("[{level}] {text}"));
            }
            Ok(JsValue::undefined())
        });
        console_obj.set(
            js_string!(level),
            JsValue::from(f.to_js_function(&realm)),
            false,
            context,
        )?;
    }
    context.global_object().set(
        js_string!("console"),
        JsValue::from(console_obj),
        false,
        context,
    )?;
    Ok(())
}

/// Installs a `performance.now()` monotonic clock.
fn install_performance(context: &mut Context, start: std::time::Instant) -> JsResult<()> {
    let now_fn = NativeFunction::from_copy_closure(move |_this, _args, _ctx| {
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        Ok(JsValue::from(elapsed))
    });
    let perf_obj = JsObject::with_null_proto();
    let realm = context.realm().clone();
    perf_obj.set(
        js_string!("now"),
        JsValue::from(now_fn.to_js_function(&realm)),
        false,
        context,
    )?;
    context.global_object().set(
        js_string!("performance"),
        JsValue::from(perf_obj),
        false,
        context,
    )?;
    Ok(())
}

/// Result of [`prepare`].
pub struct PreparedTest {
    /// The isolated context.
    pub context: Context,
    /// Shared harness state used by the native bridge.
    pub state: Arc<Mutex<HarnessState>>,
}

/// Creates a fresh isolated context for one WPT test file.
///
/// * a new `Context` with the `IndexedDbExtension` (on the selected backend,
///   isolated under `storage_dir`);
/// * the browser polyfills; and
/// * the native `testharness.js` reporters.
pub fn prepare(backend: Backend, storage_dir: &Path) -> JsResult<PreparedTest> {
    let mut context = Context::default();

    let extension = match backend {
        Backend::Memory => IndexedDbExtension::builder()
            .storage_key(StorageKey::new("http://localhost"))
            .backend_factory(Arc::new(MemoryBackendFactory::new()))
            .build()?,
        Backend::Sqlite => IndexedDbExtension::builder()
            .storage_key(StorageKey::new("http://localhost"))
            .backend_factory(Arc::new(SqliteBackendFactory::new(storage_dir)))
            .build()?,
    };
    extension.register(&mut context)?;

    // The runner drives `drive_turn` itself (fair scheduling with promise
    // jobs and virtual timers): no pump jobs are ever scheduled, so
    // `run_jobs` always terminates.
    boa_idb::runtime::set_auto_pump(&mut context, false);

    install_timers(&mut context)?;

    let state = Arc::new(Mutex::new(HarnessState::default()));
    install_js_polyfills(&mut context)?;
    install_structured_clone(&mut context)?;
    install_message_channel(&mut context)?;
    install_console(&mut context, &state)?;
    install_performance(&mut context, std::time::Instant::now())?;

    Ok(PreparedTest { context, state })
}

/// Loads and evaluates the official `testharness.js`, `testharnessreport.js`
/// and installs the completion bridge.
pub fn install_harness(
    context: &mut Context,
    resources_dir: &Path,
    state: &Arc<Mutex<HarnessState>>,
) -> JsResult<()> {
    let harness_js = std::fs::read(resources_dir.join("testharness.js")).map_err(|e| {
        JsNativeError::error().with_message(format!("cannot read testharness.js: {e}"))
    })?;
    context.eval(boa_engine::Source::from_bytes(&harness_js))?;

    if let Ok(report_js) = std::fs::read(resources_dir.join("testharnessreport.js")) {
        let _ = context.eval(boa_engine::Source::from_bytes(&report_js));
    }

    install_wpt_reporter(context, state)
}

/// Evaluates a script string against the context, returning a readable error on failure.
pub fn eval_script(context: &mut Context, name: &str, script: &str) -> JsResult<()> {
    context
        .eval(boa_engine::Source::from_bytes(script))
        .map(|_| ())
        .map_err(|e| {
            JsNativeError::error()
                .with_message(format!("{name}: {e}"))
                .into()
        })
}
