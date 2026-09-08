//! `testharness.js` ↔ Rust bridge.
//!
//! Loads the official `testharness.js` harness, exposes the native reporters
//! `__wpt_report_completion`, `__wpt_report_subtest`, `__wpt_report_registered`
//! and binds `add_completion_callback` to stream the completion report over the
//! native bridge.
//!
//! The shared sink is stored in the Context host-defined data, so the
//! native functions are capture-free (`from_fn_ptr`).

use boa_engine::native_function::NativeFunction;
use boa_engine::{Context, JsResult, JsValue, js_string};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::report::{SubtestResult, SubtestStatus, WptRunResult};

/// Shared state between the JS harness bridge and the Rust runner.
#[derive(Debug, Default)]
pub struct HarnessState {
    /// Set by `add_completion_callback` bridge (`__wpt_report_completion`).
    pub completion: Option<WptRunResult>,
    /// Live results streamed by `add_result_callback` (`__wpt_report_subtest`).
    pub results: BTreeMap<String, SubtestResult>,
    /// Names of every registered subtest, in registration order.
    pub registered: Vec<String>,
    /// Captured diagnostics (console / error messages).
    pub messages: Vec<String>,
}

impl HarnessState {
    /// Records a live subtest result.
    pub fn record_subtest(&mut self, name: String, status: SubtestStatus, message: Option<String>) {
        self.results.insert(
            name.clone(),
            SubtestResult {
                name: name.clone(),
                status,
                message,
            },
        );
        self.record_registered(name);
    }

    /// Records a registered-but-not-yet-completed subtest name.
    pub fn record_registered(&mut self, name: String) {
        if !self.registered.contains(&name) {
            self.registered.push(name);
        }
    }
}

/// `{ "name", "status", "message" }` payload from `add_result_callback`.
#[derive(Debug, serde::Deserialize)]
struct SubtestPayload {
    name: String,
    status: u8,
    message: Option<String>,
}

/// `{ "name" }` payload from `add_test_state_callback`.
#[derive(Debug, serde::Deserialize)]
struct RegisteredPayload {
    name: String,
}

/// Reads the first argument as a JS string.
fn arg_as_string(args: &[JsValue]) -> Option<String> {
    args.first()?.as_string().map(|s| s.to_std_string_escaped())
}

/// Retrieves the shared harness state sink from the context.
pub fn state_sink(context: &Context) -> Option<Arc<Mutex<HarnessState>>> {
    context.get_data::<Arc<Mutex<HarnessState>>>().cloned()
}

/// Installs the native WPT reporters and binds the `testharness.js` completion
/// callbacks. Must be called after `testharness.js` has been evaluated.
pub fn install_wpt_reporter(
    context: &mut Context,
    state: &Arc<Mutex<HarnessState>>,
) -> JsResult<()> {
    context.insert_data(Arc::clone(state));

    // `__wpt_report_completion(json)` — final run report.
    let reporter_c = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        if let Some(json) = arg_as_string(args)
            && let Some(sink) = state_sink(ctx)
        {
            match serde_json::from_str::<WptRunResult>(&json) {
                Ok(report) => {
                    sink.lock().completion = Some(report);
                }
                Err(e) => {
                    sink.lock()
                        .messages
                        .push(format!("completion JSON parse error: {e}"));
                }
            }
        }
        Ok(JsValue::undefined())
    });
    context.global_object().set(
        js_string!("__wpt_report_completion"),
        JsValue::from(reporter_c.to_js_function(context.realm())),
        false,
        context,
    )?;

    // `__wpt_report_subtest(json)` — live subtest results.
    let reporter_s = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        if let Some(json) = arg_as_string(args)
            && let Ok(payload) = serde_json::from_str::<SubtestPayload>(&json)
            && let Some(sink) = state_sink(ctx)
        {
            sink.lock().record_subtest(
                payload.name,
                SubtestStatus::from_wpt_code(payload.status),
                payload.message,
            );
        }
        Ok(JsValue::undefined())
    });
    context.global_object().set(
        js_string!("__wpt_report_subtest"),
        JsValue::from(reporter_s.to_js_function(context.realm())),
        false,
        context,
    )?;

    // `__wpt_report_registered(json)` — registered subtest names.
    let reporter_r = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        if let Some(json) = arg_as_string(args)
            && let Ok(payload) = serde_json::from_str::<RegisteredPayload>(&json)
            && let Some(sink) = state_sink(ctx)
        {
            sink.lock().record_registered(payload.name);
        }
        Ok(JsValue::undefined())
    });
    context.global_object().set(
        js_string!("__wpt_report_registered"),
        JsValue::from(reporter_r.to_js_function(context.realm())),
        false,
        context,
    )?;

    // JS bridge: bind `testharness.js` callbacks to the native reporters.
    context.eval(boa_engine::Source::from_bytes(
        r"
        (function() {
            function safe_json(obj) {
                try { return JSON.stringify(obj); } catch (e) { return null; }
            }
            if (typeof add_completion_callback === 'function') {
                add_completion_callback(function(tests, status, _asserts) {
                    var report = {
                        status: (status && typeof status.status === 'number') ? status.status : 0,
                        message: (status && typeof status.message === 'string') ? status.message : null,
                        subtests: tests.map(function(t) {
                            return {
                                name: t.name,
                                status: (typeof t.status === 'number') ? t.status : 1,
                                message: (typeof t.message === 'string') ? t.message : null
                            };
                        })
                    };
                    __wpt_report_completion(safe_json(report));
                });
            }
            if (typeof add_result_callback === 'function') {
                add_result_callback(function(t) {
                    __wpt_report_subtest(
                        safe_json({ name: t.name, status: t.status, message: (typeof t.message === 'string') ? t.message : null })
                    );
                });
            }
            if (typeof add_test_state_callback === 'function') {
                add_test_state_callback(function(t) {
                    __wpt_report_registered(
                        safe_json({ name: t.name })
                    );
                });
            }
        })();
        ",
    ))?;

    Ok(())
}
