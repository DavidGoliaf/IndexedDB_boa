//! CLI entry point for the `boa-idb-wpt` runner.

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use colored::Colorize;

use boa_idb_wpt::environment::Backend;
use boa_idb_wpt::expectations;
use boa_idb_wpt::report::{RunSummary, SubtestStatus};
use boa_idb_wpt::runner::{self, RunnerOptions};

/// Command-line arguments.
#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(
    name = "boa-idb-wpt",
    about = "W3C Web Platform Tests runner for boa_idb"
)]
struct Cli {
    /// Storage backend: `memory` or `sqlite`.
    #[arg(long, default_value = "memory")]
    backend: String,

    /// Only run test files whose path contains this substring.
    #[arg(long)]
    filter: Option<String>,

    /// Number of parallel workers (default: one deterministic worker).
    #[arg(long, default_value_t = 0)]
    threads: usize,

    /// Directory with the WPT test suite (default: `<crate>/wpt/IndexedDB`).
    #[arg(long)]
    wpt_dir: Option<PathBuf>,

    /// Directory with `testharness.js` / `testharnessreport.js`.
    #[arg(long)]
    resources_dir: Option<PathBuf>,

    /// Path to `expectations.json` (default: `<crate>/expectations.json`).
    #[arg(long)]
    expectations: Option<PathBuf>,

    /// Update `expectations.json` from the latest run.
    #[arg(long)]
    update_expectations: bool,

    /// Verify the run against `expectations.json` (0 unexpected regressions).
    #[arg(long)]
    check_expectations: bool,

    /// Print the aggregated summary and per-file head/tail.
    #[arg(long)]
    summary: bool,

    /// Per-file wall-clock budget in seconds.
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Do not print per-file progress.
    #[arg(long)]
    quiet: bool,
}

fn main() {
    let cli = Cli::parse();

    let backend = Backend::parse(&cli.backend).unwrap_or_else(|| {
        eprintln!(
            "{} unknown backend '{}' (expected 'memory' or 'sqlite')",
            "error:".red().bold(),
            cli.backend
        );
        std::process::exit(2);
    });

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let wpt_dir = cli
        .wpt_dir
        .unwrap_or_else(|| manifest_dir.join("wpt").join("IndexedDB"));
    let resources_dir = cli
        .resources_dir
        .unwrap_or_else(|| manifest_dir.join("wpt").join("resources"));
    let expectations_path = cli
        .expectations
        .unwrap_or_else(|| manifest_dir.join("expectations.json"));

    // The default suite is intentionally deterministic: several WPT tests
    // exercise global database/version-change ordering, and parallel workers
    // make timeout-based completion races observable. Parallel execution is
    // still available explicitly through `--threads N`.
    let threads = if cli.threads == 0 { 1 } else { cli.threads };

    let options = RunnerOptions {
        backend,
        filter: cli.filter.clone(),
        threads,
        file_timeout: Duration::from_secs(cli.timeout),
        resources_dir: Some(resources_dir.clone()),
        quiet: cli.quiet,
    };

    let report = match runner::run_suite(&wpt_dir, &options) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(2);
        }
    };

    // `--check-expectations`: compare with the committed snapshot.
    let mut had_regressions = has_fatal_run_status(&report.summary);
    if cli.check_expectations {
        match expectations::load(&expectations_path) {
            Ok(snapshot) => {
                let outcome = expectations::check(&report.files, &snapshot);
                println!(
                    "check expectations: {} subtests, {} matched, {} unexpected",
                    outcome.total,
                    outcome.matched,
                    outcome.unexpected.len()
                );
                for unexpected in &outcome.unexpected {
                    println!("  {unexpected}");
                }
                // A matching expectation does not make a timeout or NOTRUN
                // successful. Those statuses are always fatal for the CLI;
                // the snapshot result only contributes additional failures.
                had_regressions |= outcome.regression_count() > 0;
            }
            Err(e) => {
                eprintln!("{} cannot read expectations: {}", "error:".red().bold(), e);
                had_regressions = true;
            }
        }
    }

    // `--update-expectations`: write a fresh snapshot.
    if cli.update_expectations {
        let snapshot = expectations::build_expectations(&report.files);
        match expectations::save(&expectations_path, &snapshot) {
            Ok(()) => println!("wrote {}", expectations_path.display()),
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(2);
            }
        }
    }

    print_results(&report);

    if had_regressions {
        std::process::exit(1);
    }
}

/// Returns whether the run contains a terminal status that must fail the CLI.
///
/// TIMEOUT and NOTRUN are infrastructure/protocol failures, not functional
/// results. They remain fatal even when an expectations snapshot records the
/// same status.
fn has_fatal_run_status(summary: &RunSummary) -> bool {
    summary.timed_out > 0 || summary.not_run > 0
}

/// Renders the per-file result lines and the aggregate summary.
fn print_results(report: &runner::RunReport) {
    let mut pass_files = 0usize;
    let mut fail_files = 0usize;

    for file in &report.files {
        let failed = file
            .subtests()
            .iter()
            .any(|s| s.status != SubtestStatus::Pass);
        if failed {
            fail_files += 1;
            println!("{} {}", "FAIL".red().bold(), file.file);
        } else {
            pass_files += 1;
            println!("{} {}", "PASS".green().bold(), file.file);
        }
        if !failed {
            continue;
        }
        // Show up to 5 failing subtests per file for quick diagnosis.
        let failures: Vec<_> = file
            .subtests()
            .into_iter()
            .filter(|s| s.status != SubtestStatus::Pass)
            .collect();
        for sub in failures.iter().take(5) {
            println!(
                "    [{}] {}{}",
                subtest_status_color(sub.status),
                sub.name,
                sub.message
                    .as_deref()
                    .map(|m| format!(" — {}", first_line(m)))
                    .unwrap_or_default()
            );
        }
        if failures.len() > 5 {
            println!("    … and {} more failures", failures.len() - 5);
        }
    }

    let s = &report.summary;
    println!();
    let rate = s.pass_rate();
    eprintln!(
        "{} {} files: {} PASS ({:.1}%), {} FAIL",
        "SUMMARY".bold(),
        pass_files + fail_files,
        s.passed,
        rate,
        s.failed
    );
}

/// First line of a message for compact CLI output.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).to_string()
}

/// Colored subtest status label for the CLI.
fn subtest_status_color(status: SubtestStatus) -> String {
    match status {
        SubtestStatus::Pass => "PASS".green().to_string(),
        SubtestStatus::Fail => "FAIL".red().to_string(),
        SubtestStatus::Timeout => "TIMEOUT".yellow().to_string(),
        SubtestStatus::NotRun => "NOTRUN".yellow().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::has_fatal_run_status;
    use boa_idb_wpt::report::RunSummary;

    #[test]
    fn timeout_remains_fatal_when_snapshot_matches() {
        let summary = RunSummary {
            total: 1,
            timed_out: 1,
            ..RunSummary::default()
        };

        assert!(has_fatal_run_status(&summary));
    }

    #[test]
    fn ordinary_failures_are_left_to_expectation_check() {
        let summary = RunSummary {
            total: 1,
            failed: 1,
            ..RunSummary::default()
        };

        assert!(!has_fatal_run_status(&summary));
    }
}
