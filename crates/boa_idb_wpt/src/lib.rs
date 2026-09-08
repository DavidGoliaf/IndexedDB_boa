//! W3C Web Platform Tests runner for `boa_idb`.
//!
//! The runner executes the official `wpt/IndexedDB/**` suite against the
//! `boa_idb` implementation on either backend (`memory` / `sqlite`) without a
//! browser or Node.js: every test file runs in an isolated Boa `Context`
//! with browser polyfills and a native `testharness.js` bridge.
//!
//! See the crate README for the pipeline role and usage examples.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod environment;
pub mod expectations;
pub mod harness;
pub mod report;
pub mod runner;

pub use report::{FileReport, RunSummary, SubtestResult, SubtestStatus, WptRunResult};
