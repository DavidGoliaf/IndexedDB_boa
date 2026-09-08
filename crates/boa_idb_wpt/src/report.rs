//! Report structures for WPT runs (`TestFileResult`, `SubtestResult`, `Status`).
//!
//! The JS side (`testharness.js`) reports through the native bridge as a JSON
//! object serialized with `WptRunResult`; the runner aggregates per-file results
//! into [`FileReport`] and computes [`RunSummary`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Status of a single subtest (mirrors `testharness.js` `Test.status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SubtestStatus {
    /// Subtest passed.
    Pass,
    /// Subtest failed.
    Fail,
    /// Subtest timed out.
    Timeout,
    /// Subtest was never run.
    NotRun,
}

impl SubtestStatus {
    /// Returns the lowercase string form as produced by `testharness.js`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Timeout => "TIMEOUT",
            Self::NotRun => "NOTRUN",
        }
    }

    /// Parses a `testharness.js` test status byte.
    /// `0` = pass, `1` = fail, `2` = timeout, `3` = notrun.
    #[must_use]
    pub fn from_wpt_code(code: u8) -> Self {
        match code {
            1 => Self::Fail,
            2 => Self::Timeout,
            3 => Self::NotRun,
            _ => Self::Pass,
        }
    }

    /// The numeric status code used by `testharness.js`.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Fail => 1,
            Self::Timeout => 2,
            Self::NotRun => 3,
        }
    }
}

/// Result of a single subtest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtestResult {
    /// Subtest name as registered with `test()/async_test()/promise_test()`.
    pub name: String,
    /// Subtest status.
    pub status: SubtestStatus,
    /// Optional failure message.
    pub message: Option<String>,
}

/// Harness-level run result as reported by `testharness.js` over the native
/// bridge (`__wpt_report_completion`).
///
/// `status` uses the `testharness.js` harness status codes:
/// `0` = OK, `1` = ERROR, `2` = TIMEOUT, `3` = `PRECONDITION_FAILED`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WptRunResult {
    /// Harness status code.
    pub status: u8,
    /// Harness-level message (usually `None`).
    pub message: Option<String>,
    /// All subtests that were registered.
    pub subtests: Vec<SubtestResult>,
}

/// A single reported subtest, also carrying the file it belongs to.
#[derive(Debug, Clone)]
pub struct ReportedSubtest {
    /// File name (relative to the WPT root).
    pub file: String,
    /// Subtest name.
    pub name: String,
    /// Subtest status.
    pub status: SubtestStatus,
    /// Failure message.
    pub message: Option<String>,
}

/// Complete outcome for one WPT file.
#[derive(Debug, Clone)]
pub struct FileReport {
    /// File name relative to the WPT root (the key used in `expectations.json`).
    pub file: String,
    /// The raw harness result (`None` when the file was force-finalized).
    pub result: Option<WptRunResult>,
    /// True when `add_completion_callback` fired inside the budget.
    pub completed: bool,
    /// Human-readable reason when the file did not complete normally.
    pub forced_reason: Option<String>,
}

impl FileReport {
    /// The subtests this file produced; when the file stalled the runner
    /// supplies a deterministic `Fail` entry instead of a timeout.
    #[must_use]
    pub fn subtests(&self) -> Vec<ReportedSubtest> {
        let mut out = Vec::new();
        if let Some(result) = &self.result {
            for sub in &result.subtests {
                out.push(ReportedSubtest {
                    file: self.file.clone(),
                    name: sub.name.clone(),
                    status: sub.status,
                    message: sub.message.clone(),
                });
            }
        }
        out
    }

    /// Adds a deterministic failed subtest entry (used for setup failures).
    pub fn push_forced_subtest(&mut self, name: String, message: String) {
        self.push_forced_subtest_with_status(name, SubtestStatus::Fail, message);
    }

    /// Adds a deterministic terminal subtest entry.
    pub fn push_forced_subtest_with_status(
        &mut self,
        name: String,
        status: SubtestStatus,
        message: String,
    ) {
        let entry = SubtestResult {
            name,
            status,
            message: Some(message),
        };
        match self.result.as_mut() {
            Some(r) => r.subtests.push(entry),
            None => {
                self.result = Some(WptRunResult {
                    status: status.code(),
                    message: self.forced_reason.clone(),
                    subtests: vec![entry],
                });
            }
        }
    }
}

/// Aggregate counts over a set of subtest results.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RunSummary {
    /// Total number of subtests.
    pub total: u64,
    /// Subtests that passed.
    pub passed: u64,
    /// Subtests that failed.
    pub failed: u64,
    /// Subtests that timed out before file completion.
    pub timed_out: u64,
    /// Subtests that were not run.
    pub not_run: u64,
}

impl RunSummary {
    /// Updates the summary from one subtest result.
    pub fn record(&mut self, status: SubtestStatus) {
        self.total += 1;
        match status {
            SubtestStatus::Pass => self.passed += 1,
            SubtestStatus::Fail => self.failed += 1,
            SubtestStatus::Timeout => self.timed_out += 1,
            SubtestStatus::NotRun => self.not_run += 1,
        }
    }

    /// Pass rate as a percentage of the total (0.0 when there are no subtests).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.passed as f64 * 100.0 / self.total as f64
        }
    }
}

/// Per-file expectations (from `expectations.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileExpectation {
    /// Expected status per subtest name.
    pub subtests: BTreeMap<String, SubtestStatus>,
    /// Reason why the file is not fully passing.
    pub reason: Option<String>,
}

/// The complete expectations document.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Expectations {
    /// Expected statuses keyed by WPT file name (relative to `wpt/IndexedDB/`).
    pub files: BTreeMap<String, FileExpectation>,
}

impl Expectations {
    /// Loads expectations from a JSON blob.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let with_defaults: Expectations = serde_json::from_str::<Expectations>(json)
            .or_else(|_| serde_json::from_str::<Expectations>(&format!(r#"{{"files":{json}}}"#)))?;
        Ok(with_defaults)
    }

    /// Serializes expectations to a JSON blob.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Statistics about a full run of Expectations.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExpectationsStats {
    /// Files that had expectations.
    pub files: u64,
    /// Subtests that passed matches expectation.
    pub matched: u64,
    /// Subtests that diverged from the registered expectation.
    pub unmatched: u64,
    /// Total subtest expectations.
    pub total: u64,
}

/// Relative path helper: normalizes `wpt/IndexedDB/foo.any.js` to `foo.any.js`.
pub fn file_key(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).map_or_else(
        |_| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().to_string(),
            )
        },
        |p| p.to_string_lossy().replace('\\', "/"),
    )
}
