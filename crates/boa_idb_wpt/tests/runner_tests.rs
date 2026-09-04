//! Focused tests for WPT discovery and expectation comparison.

use std::fs;

use boa_idb_wpt::expectations::{build_expectations, check};
use boa_idb_wpt::report::{FileReport, SubtestResult, SubtestStatus, WptRunResult};
use boa_idb_wpt::runner::discover;
use tempfile::tempdir;

#[test]
fn discover_filters_test_files_and_skips_resources() {
    let dir = tempdir().expect("temporary directory");
    fs::create_dir_all(dir.path().join("resources")).expect("resources directory");
    fs::write(dir.path().join("alpha.any.js"), "").expect("alpha");
    fs::write(dir.path().join("beta.html"), "").expect("beta");
    fs::write(dir.path().join("resources/ignored.any.js"), "").expect("ignored");
    fs::write(dir.path().join("notes.txt"), "").expect("notes");

    let files = discover(dir.path(), Some("alpha"));
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].file_name().and_then(|n| n.to_str()),
        Some("alpha.any.js")
    );
}

#[test]
fn expectations_round_trip_and_detect_pass_regressions() {
    let report = FileReport {
        file: "alpha.any.js".into(),
        result: Some(WptRunResult {
            status: 0,
            message: None,
            subtests: vec![SubtestResult {
                name: "works".into(),
                status: SubtestStatus::Pass,
                message: None,
            }],
        }),
        completed: true,
        forced_reason: None,
    };
    let expectations = build_expectations(std::slice::from_ref(&report));
    assert_eq!(check(&[report], &expectations).unexpected.len(), 0);
}

#[test]
fn expectations_are_strict_about_status_and_shape() {
    let report = FileReport {
        file: "alpha.any.js".into(),
        result: Some(WptRunResult {
            status: 0,
            message: None,
            subtests: vec![SubtestResult {
                name: "works".into(),
                status: SubtestStatus::Pass,
                message: None,
            }],
        }),
        completed: true,
        forced_reason: None,
    };
    let mut expectations = build_expectations(std::slice::from_ref(&report));
    expectations
        .files
        .get_mut("alpha.any.js")
        .expect("file")
        .subtests
        .insert("extra".into(), SubtestStatus::Pass);
    expectations
        .files
        .get_mut("alpha.any.js")
        .expect("file")
        .subtests
        .insert("works".into(), SubtestStatus::Fail);
    let outcome = check(&[report], &expectations);
    assert_eq!(outcome.unexpected.len(), 2);
}

#[test]
fn forced_timeout_keeps_timeout_status() {
    let mut report = FileReport {
        file: "slow.any.js".into(),
        result: None,
        completed: false,
        forced_reason: Some("file budget exhausted".into()),
    };
    report.push_forced_subtest_with_status(
        "slow.any.js (harness)".into(),
        SubtestStatus::Timeout,
        "file budget exhausted".into(),
    );
    assert_eq!(report.subtests()[0].status, SubtestStatus::Timeout);
}
