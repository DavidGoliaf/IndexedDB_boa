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
