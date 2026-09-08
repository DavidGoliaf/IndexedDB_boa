//! IndexedDB API 3.0 implementation for the Boa JavaScript engine.

#![deny(unsafe_code)]
#![warn(missing_docs)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::unnecessary_wraps,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::struct_excessive_bools,
    clippy::match_same_arms,
    clippy::new_without_default,
    clippy::return_self_not_must_use,
    clippy::redundant_closure_for_method_calls,
    clippy::collapsible_if,
    dead_code,
    unused_variables,
    unused_imports,
    missing_docs
)]

pub mod api;
pub mod convert;
pub mod dom;
pub mod driver;
pub mod engine;
pub mod executor;
pub mod extension;
pub mod io;
pub mod observer;
pub mod runtime;
