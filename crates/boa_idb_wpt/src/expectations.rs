//! Loading, building, checking and updating `expectations.json`.
//!
//! Every non-passing subtest is deterministically recorded here together with
//! a justification. Timeout and not-run states remain distinct from failures.

use std::fs;
use std::path::Path;

use crate::report::{Expectations, FileExpectation, FileReport, RunSummary, SubtestStatus};

/// Error while working with expectations.
#[derive(thiserror::Error, Debug)]
pub enum ExpectationError {
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The path that failed.
        path: String,
        /// The IO error.
        #[source]
        source: std::io::Error,
    },
    /// The file had invalid JSON.
    #[error("invalid JSON in {path}: {source}")]
    Json {
        /// The path that failed.
        path: String,
        /// The parse error.
        #[source]
        source: serde_json::Error,
    },
}

/// Loads expectations from a JSON file.
pub fn load(path: &Path) -> Result<Expectations, ExpectationError> {
    let text = fs::read_to_string(path).map_err(|source| ExpectationError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let parsed: Expectations =
        serde_json::from_str(&text).map_err(|source| ExpectationError::Json {
            path: path.display().to_string(),
            source,
        })?;
    Ok(parsed)
}

/// Persists expectations to a JSON file.
pub fn save(path: &Path, expectations: &Expectations) -> Result<(), ExpectationError> {
    let text =
        serde_json::to_string_pretty(expectations).map_err(|source| ExpectationError::Json {
            path: path.display().to_string(),
            source,
        })?;
    fs::write(path, format!("{text}\n")).map_err(|source| ExpectationError::Io {
        path: path.display().to_string(),
        source,
    })
}

/// Builds an expectations snapshot from a set of file reports.
///
/// Subtests that are not passing are recorded with `reason` describing the
/// failure cause so every entry is auditable.
pub fn build_expectations(reports: &[FileReport]) -> Expectations {
    let mut expectations = Expectations::default();
    for report in reports {
        let mut subtests = std::collections::BTreeMap::new();
        for sub in report.subtests() {
            subtests.insert(sub.name, sub.status);
        }
        if subtests.is_empty() {
            continue;
        }
        let reason = if report.completed {
            aggregate_reason(report)
        } else {
            report.forced_reason.clone()
        };
        expectations
            .files
            .insert(report.file.clone(), FileExpectation { subtests, reason });
    }
    expectations
}

/// Summarizes the first failure messages found in a report for documentation.
fn aggregate_reason(report: &FileReport) -> Option<String> {
    let mut reasons = Vec::new();
    if let Some(result) = &report.result {
        for sub in &result.subtests {
            if sub.status != SubtestStatus::Pass
                && let Some(message) = &sub.message
            {
                let name = if sub.name.len() > 60 {
                    format!("{}…", &sub.name[..60])
                } else {
                    sub.name.clone()
                };
                reasons.push(format!("{name}: {}", first_line(message)));
            }
        }
    }
    if let Some(reason) = &report.forced_reason {
        reasons.push(first_line(reason).to_string());
    }
    reasons.truncate(3);
    if reasons.is_empty() {
        None
    } else {
        Some(reasons.join(" | "))
    }
}

/// First non-empty line of a message, truncated.
fn first_line(message: &str) -> &str {
    let line = message.lines().next().unwrap_or(message);
    if line.len() > 160 { &line[..160] } else { line }
}

/// Outcome of a `--check-expectations` run.
#[derive(Debug, Default)]
pub struct CheckOutcome {
    /// Total subtests compared.
    pub total: u64,
    /// Matched expectations.
    pub matched: u64,
    /// Unexpected deviations from the recorded expectations.
    pub unexpected: Vec<String>,
}

impl CheckOutcome {
    /// Number of unanticipated regressions.
    #[must_use]
    pub fn regression_count(&self) -> u64 {
        self.unexpected.len() as u64
    }
}

/// Compares a run against committed expectations.
///
/// A subtest that *passes* is never a regression even if its expectation says
/// otherwise; only an unexpected *failure* (or hang) against a recorded pass is
/// a regression. Failures with a matching `Fail` expectation are expected.
pub fn check(reports: &[FileReport], expectations: &Expectations) -> CheckOutcome {
    let mut outcome = CheckOutcome::default();
    let mut seen = std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for report in reports {
        for sub in report.subtests() {
            outcome.total += 1;
            seen.entry(sub.file.clone())
                .or_default()
                .insert(sub.name.clone());
            let expected = expectations
                .files
                .get(&sub.file)
                .and_then(|f| f.subtests.get(&sub.name));
            match expected {
                Some(expected_status) => {
                    if sub.status == *expected_status {
                        outcome.matched += 1;
                    } else {
                        outcome.unexpected.push(format!(
                            "{} :: {} expected {}, got {} (message: {})",
                            sub.file,
                            sub.name,
                            expected_status.as_str(),
                            sub.status.as_str(),
                            sub.message.clone().unwrap_or_default()
                        ));
                    }
                }
                None => {
                    outcome.unexpected.push(format!(
                        "{} :: {} has no expectation, got {} (message: {})",
                        sub.file,
                        sub.name,
                        sub.status.as_str(),
                        sub.message.clone().unwrap_or_default()
                    ));
                }
            }
        }
    }
    for (file, expectation) in &expectations.files {
        let Some(actual) = seen.get(file) else {
            outcome
                .unexpected
                .push(format!("{file} was expected but did not run"));
            continue;
        };
        for name in expectation.subtests.keys() {
            if !actual.contains(name) {
                outcome
                    .unexpected
                    .push(format!("{file} :: {name} was expected but did not run"));
            }
        }
    }
    outcome
}

/// Renders a run summary line for the CLI.
pub fn summarize(report: &RunSummary) -> String {
    format!(
        "{} subtests: {} PASS ({}%), {} FAIL, {} TIMEOUT, {} NOTRUN",
        report.total,
        report.passed,
        report.pass_rate().round(),
        report.failed,
        report.timed_out,
        report.not_run
    )
}
