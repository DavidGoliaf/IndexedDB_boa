//! Parallel / sequential WPT runner over `wpt/IndexedDB/**`.
//!
//! Every test file runs in its own isolated `Context` with a fresh temp
//! storage; the runner drives the event loop (micro-tasks + macro-tasks) and
//! force-finalizes files that stall so no subtest is ever reported as
//! CRASH/TIMEOUT.

use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::environment::{self, Backend};
use crate::harness::HarnessState;
use crate::report::{FileReport, RunSummary, SubtestResult, SubtestStatus, WptRunResult, file_key};

/// Options controlling one suite run.
#[derive(Debug, Clone)]
pub struct RunnerOptions {
    /// Storage backend.
    pub backend: Backend,
    /// Optional substring filter on the file key.
    pub filter: Option<String>,
    /// Number of worker threads (parallel run).
    pub threads: usize,
    /// Wall-clock budget for one file.
    pub file_timeout: Duration,
    /// Directory with `testharness.js` / `testharnessreport.js`.
    pub resources_dir: Option<PathBuf>,
    /// Disable progress printing.
    pub quiet: bool,
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            backend: Backend::Memory,
            filter: None,
            threads: 1,
            file_timeout: Duration::from_secs(30),
            resources_dir: None,
            quiet: false,
        }
    }
}

/// Aggregated outcome of a suite run.
#[derive(Debug, Default)]
pub struct RunReport {
    /// One report per executed file.
    pub files: Vec<FileReport>,
    /// Aggregate subtest counts.
    pub summary: RunSummary,
}

/// Maximum macro-task turns before a file is considered stalled.
const MAX_TURNS: u64 = 500_000;

/// What a drain pass observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainOutcome {
    /// The completion callback fired.
    Completed,
    /// Everything settled without the completion callback.
    Quiescent,
    /// Budget was exhausted while work remained.
    BudgetExhausted,
}

/// Discovers WPT test files under `wpt_root` (`*.any.js`, `*.htm`, `*.html`),
/// skipping `resources/` directories and unknown files.
pub fn discover(wpt_root: &Path, filter: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(wpt_root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(wpt_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel.contains("/resources/") {
            continue;
        }
        // WPT file names are lowercase by convention; a case-sensitive match
        // is intentional here.
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let is_test = rel.ends_with(".any.js") || rel.ends_with(".htm") || rel.ends_with(".html");
        if !is_test {
            continue;
        }
        if let Some(filter) = filter
            && !rel.contains(filter)
        {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    out
}

/// Runs a whole suite; never fails per-file, errors are folded into reports.
///
/// # Errors
///
/// Returns an error only when the root does not exist or no files match.
pub fn run_suite(wpt_root: &Path, options: &RunnerOptions) -> std::io::Result<RunReport> {
    if !wpt_root.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("WPT root not found: {}", wpt_root.display()),
        ));
    }
    let files = discover(wpt_root, options.filter.as_deref());
    if files.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no WPT test files matched",
        ));
    }

    let resources = options.resources_dir.clone().unwrap_or_else(|| {
        wpt_root
            .parent()
            .map_or_else(|| wpt_root.join("resources"), |p| p.join("resources"))
    });
    let resources_ref = resources.as_path();
    let queue: Arc<Mutex<VecDeque<PathBuf>>> = Arc::new(Mutex::new(files.clone().into()));
    let next = Arc::new(AtomicUsize::new(0));
    let total = files.len();
    let reports: Arc<Mutex<Vec<Option<FileReport>>>> =
        Arc::new(Mutex::new((0..total).map(|_| None).collect()));

    let workers = options.threads.clamp(1, total);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let next = Arc::clone(&next);
            let reports = Arc::clone(&reports);
            let builder = std::thread::Builder::new()
                .name("wpt-worker".to_string())
                // Boa's VM needs a large native stack for deep scripts (e.g.
                // testharness.js) — the default 1 MiB overflows.
                .stack_size(64 * 1024 * 1024);
            let handle = builder.spawn_scoped(scope, move || {
                loop {
                    let path = {
                        let mut q = queue.lock();
                        q.pop_front()
                    };
                    let Some(path) = path else { break };
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if !options.quiet {
                        let key = file_key(&path, wpt_root);
                        eprintln!("[{}/{}] Starting {}", index + 1, total, key);
                    }
                    let report = run_one_file(wpt_root, resources_ref, &path, options);
                    reports.lock()[index] = Some(report);
                    if !options.quiet {
                        let key = file_key(&path, wpt_root);
                        eprintln!("[{}/{}] Finished {}", index + 1, total, key);
                    }
                }
            });
            let _ = handle;
        }
    });

    let mut files_out = Vec::with_capacity(total);
    let mut summary = RunSummary::default();
    for report in reports.lock().iter().flatten() {
        for sub in report.subtests() {
            summary.record(sub.status);
        }
        files_out.push(report.clone());
    }

    Ok(RunReport {
        files: files_out,
        summary,
    })
}

/// Runs a single test file in an isolated context.
fn run_one_file(
    wpt_root: &Path,
    resources: &Path,
    path: &Path,
    options: &RunnerOptions,
) -> FileReport {
    let key = file_key(path, wpt_root);
    let deadline = Instant::now() + options.file_timeout;

    let source = match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(e) => {
            return forced_fail_report(&key, format!("cannot read test file: {e}"));
        }
    };

    let temp = match tempfile::TempDir::new() {
        Ok(t) => t,
        Err(e) => {
            return forced_fail_report(&key, format!("cannot create temp dir: {e}"));
        }
    };

    let prepared = match environment::prepare(options.backend, temp.path()) {
        Ok(p) => p,
        Err(e) => {
            return forced_fail_report(&key, format!("environment setup failed: {e}"));
        }
    };
    let mut context = prepared.context;
    let state = prepared.state;

    // 1. testharness.js + native reporters
    if let Err(e) = environment::install_harness(&mut context, resources, &state) {
        return finalize_report(
            &key,
            &state,
            false,
            Some(format!("testharness.js failed to load: {e}")),
        );
    }

    // 2. support scripts referenced by `// META: script=...`
    if let Err(e) = eval_support_scripts(&mut context, wpt_root, path, &source) {
        return finalize_report(
            &key,
            &state,
            false,
            Some(format!("support script failed: {e}")),
        );
    }

    // 3. the test body itself
    let body_ok = match test_body(&source) {
        TestBody::Script(script) => environment::eval_script(&mut context, &key, &script),
        TestBody::Html(scripts) => {
            let mut first_err = None;
            for (n, script) in scripts.iter().enumerate() {
                if let Err(e) =
                    environment::eval_script(&mut context, &format!("{key}#{n}"), script)
                {
                    first_err = Some(e);
                    break;
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    };

    if let Err(e) = body_ok {
        return finalize_report(
            &key,
            &state,
            false,
            Some(format!("uncaught top-level error: {e}")),
        );
    }

    // 4. drive the event loop
    let outcome = pump(&mut context, &state, deadline);

    let completes = state.lock().completion.is_some();
    if !completes {
        let reason = match outcome {
            DrainOutcome::Completed => "completed".to_string(),
            DrainOutcome::Quiescent => {
                "event loop settled before completion; pending async subtests finalized as FAIL"
                    .to_string()
            }
            DrainOutcome::BudgetExhausted => {
                "file budget exhausted; pending async subtests finalized as FAIL".to_string()
            }
        };
        return finalize_report(&key, &state, false, Some(reason));
    }

    finalize_report(&key, &state, true, None)
}

/// Builds a report for a file that never got a normal completion.
fn forced_fail_report(key: &str, reason: String) -> FileReport {
    let mut report = FileReport {
        file: key.to_string(),
        result: None,
        completed: false,
        forced_reason: Some(reason.clone()),
    };
    report.push_forced_subtest(format!("{key} (harness)"), reason);
    report
}

/// Produces the final report from harness state.
fn finalize_report(
    key: &str,
    state: &Arc<Mutex<HarnessState>>,
    completed: bool,
    forced_reason: Option<String>,
) -> FileReport {
    let guard = state.lock();
    if let Some(result) = guard.completion.clone() {
        return FileReport {
            file: key.to_string(),
            result: Some(result),
            completed: true,
            forced_reason: None,
        };
    }

    let mut report = FileReport {
        file: key.to_string(),
        result: None,
        completed,
        forced_reason: forced_reason.clone(),
    };

    let registered = guard.registered.clone();
    let results = guard.results.clone();
    let messages = guard.messages.clone();

    let fallback_message = forced_reason
        .clone()
        .unwrap_or_else(|| "subtest did not complete within the file budget".to_string());

    let subtests: Vec<SubtestResult> = if registered.is_empty() {
        vec![SubtestResult {
            name: format!("{key} (harness)"),
            status: if forced_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("file budget exhausted"))
            {
                SubtestStatus::Timeout
            } else {
                SubtestStatus::Fail
            },
            message: Some(if messages.is_empty() {
                fallback_message.clone()
            } else {
                format!("{} | {}", fallback_message, messages.join(" | "))
            }),
        }]
    } else {
        registered
            .iter()
            .map(|name| {
                results.get(name).cloned().unwrap_or(SubtestResult {
                    name: name.clone(),
                    status: if forced_reason
                        .as_deref()
                        .is_some_and(|reason| reason.starts_with("file budget exhausted"))
                    {
                        SubtestStatus::Timeout
                    } else {
                        SubtestStatus::Fail
                    },
                    message: Some(if messages.is_empty() {
                        fallback_message.clone()
                    } else {
                        format!("{} | {}", fallback_message, messages.join(" | "))
                    }),
                })
            })
            .collect()
    };

    report.result = Some(WptRunResult {
        status: 1,
        message: forced_reason,
        subtests,
    });
    drop(guard);
    report
}

/// Drives the per-file event loop until completion, quiescence or budget.
fn pump(
    context: &mut boa_engine::Context,
    state: &Arc<Mutex<HarnessState>>,
    deadline: Instant,
) -> DrainOutcome {
    let mut turns = 0u64;

    loop {
        if state.lock().completion.is_some() {
            return DrainOutcome::Completed;
        }
        if turns >= MAX_TURNS || Instant::now() > deadline {
            return DrainOutcome::BudgetExhausted;
        }

        let mut did_work = false;

        // 1. Micro-tasks / promise jobs first: test bodies start deferred
        // (testharness runs them off a microtask), so the IDB work they
        // enqueue only exists after jobs run. Running jobs first keeps a
        // job-enqueued open from looking like a settled loop below.
        //
        // A throwing job aborts the file: the engine state is unknowable
        // afterwards, so there is nothing sensible to keep pumping.
        if context.run_jobs().is_err() {
            let _ = context.run_jobs();
            return DrainOutcome::Quiescent;
        }

        // 2. A single IndexedDB driver turn per loop iteration.
        //
        // The driver is drained one turn at a time (instead of looping to
        // quiescence here) so endless IDB work — e.g. a `keep_alive` spin —
        // cannot starve promise jobs and virtual timers: every turn yields
        // back to steps 1 and 3, letting `await timeoutPromise(0)` resolve
        // and the spinning test release its transaction.
        if let Ok(true) = boa_idb::driver::drive_turn(context) {
            did_work = true;
            if state.lock().completion.is_some() {
                return DrainOutcome::Completed;
            }
        }
        // A failed or idle turn carries no progress; timers below may
        // still unblock the file.

        // 3. Macro-tasks due at current virtual time (e.g. setTimeout(..., 0)).
        // A fired timer is a task boundary for IDB scopes (see
        // `boa_idb::driver::note_timer_task`).
        if let Ok(fired) = environment::fire_due_timers(context)
            && fired
        {
            did_work = true;
            boa_idb::driver::note_timer_task(context);
        }

        if state.lock().completion.is_some() {
            return DrainOutcome::Completed;
        }

        // 4. If nothing ran, advance the virtual clock to the next timer.
        if !did_work {
            if environment::advance_to_next_timer(context) {
                // Timer is now due; next loop iteration will fire it.
                turns += 1;
                continue;
            }
            // No pending timers and no IDB work: check if settled.
            let _ = context.run_jobs();
            if state.lock().completion.is_some() {
                return DrainOutcome::Completed;
            }
            return DrainOutcome::Quiescent;
        }

        turns += 1;
    }
}

/// Extracted test body: either a standalone script or inline HTML scripts.
#[derive(Debug)]
enum TestBody {
    /// Plain `.any.js` / `.js` script.
    Script(String),
    /// Inline `<script>` bodies extracted from an `.htm`/`.html` file.
    Html(Vec<String>),
}

/// Classifies and prepares the test body for evaluation.
fn test_body(source: &str) -> TestBody {
    let trimmed = source.trim_start();
    // HTML test files contain a leading doctype / html element.
    let looks_html = trimmed.starts_with("<!doctype")
        || trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || (trimmed.starts_with('<')
            && trimmed.contains("</script>")
            && !trimmed.starts_with("//"));
    if looks_html {
        TestBody::Html(extract_inline_scripts(trimmed))
    } else {
        TestBody::Script(source.to_string())
    }
}

/// Extracts inline `<script>…</script>` blocks (no `src`).
fn extract_inline_scripts(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut rest = 0;
    while rest < html.len() {
        let open_tag = match lower[rest..].find("<script") {
            Some(i) => rest + i,
            None => break,
        };
        let close_tag = match lower[open_tag..].find('>') {
            Some(i) => open_tag + i,
            None => break,
        };
        // Skip blocks with a `src` attribute.
        let attrs = &lower[open_tag + 7..close_tag];
        if attrs.contains("src") {
            let end = match lower[close_tag..].find("</script>") {
                Some(i) => close_tag + i + "</script>".len(),
                None => break,
            };
            rest = end;
            continue;
        }
        let body_start = close_tag + 1;
        let script_end = match lower[body_start..].find("</script>") {
            Some(i) => body_start + i,
            None => break,
        };
        out.push(html[body_start..script_end].to_string());
        rest = script_end + "</script>".len();
    }
    out
}

/// Evaluates support scripts listed in `// META: script=...` headers.
///
/// Nested `META` lines inside a support file resolve relative to THAT file's
/// directory (e.g. `resources/support-get-all.js` pulls in sibling
/// `nested-cloning-common.js`), not relative to the test file. Absolute
/// `/...` includes are resolved against the WPT root (the layout
/// `wpt/IndexedDB/...` + `wpt/resources/...` mirrors upstream, where
/// `/common/...` lives next to the suite directories).
fn eval_support_scripts(
    context: &mut boa_engine::Context,
    wpt_root: &Path,
    test_path: &Path,
    source: &str,
) -> Result<(), String> {
    let dir = test_path.parent().unwrap_or_else(|| Path::new("."));
    // WPT-root-relative includes (`/...`, mirroring the upstream layout where
    // `common/` sits next to the suite directories) resolve against the
    // parent of the suite root (`.../wpt/` for the default layout).
    let wpt_parent = wpt_root.parent().unwrap_or(wpt_root);
    let mut queue: VecDeque<(PathBuf, String)> = parse_meta_scripts(source)
        .into_iter()
        .map(|script| (dir.to_path_buf(), script))
        .collect();
    let mut seen = BTreeSet::new();
    let mut guard = 0;
    while let Some((base_dir, script)) = queue.pop_front() {
        guard += 1;
        if guard > 64 {
            return Err("META script chain too deep".to_string());
        }
        let resolved = if script.starts_with('/') {
            wpt_parent.join(script.trim_start_matches('/'))
        } else {
            base_dir.join(&script)
        };
        if !seen.insert(resolved.clone()) {
            continue;
        }
        let body = fs::read_to_string(&resolved)
            .map_err(|e| format!("META script {}: {e}", resolved.display()))?;
        let nested_dir = resolved
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        for nested in parse_meta_scripts(&body) {
            queue.push_back((nested_dir.clone(), nested));
        }
        if let Err(e) = environment::eval_script(context, &script, &body) {
            return Err(format!("META script {}: {e}", resolved.display()));
        }
    }
    Ok(())
}

/// Parses `// META: script=relative/path.js` header lines.
fn parse_meta_scripts(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let rest = trimmed.strip_prefix("// META:")?.trim();
            rest.strip_prefix("script=")
                .map(str::trim)
                .map(str::to_string)
        })
        .collect()
}
