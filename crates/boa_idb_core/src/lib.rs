//! Core engine and codecs for IndexedDB implementation without JS dependencies.
//!
//! This crate provides the foundational types and algorithms for an IndexedDB
//! implementation that is completely independent of any JavaScript engine.
//!
//! # Modules
//!
//! - [`error`] — Error types for keys, key paths, SCF serialization, and Idb operations.
//! - [`limits`] — Security limits and quotas configuration.
//! - [`key`] — Key types, comparison, encoding/decoding (KEY-v1), key ranges, and key paths.
//! - [`clone`] — Intermediate `ScValue` representation and SCF-v1 binary codec.
//! - [`proto`] — Protocol types, identifiers, and operation definitions.
//! - [`backend`] — Backend abstraction layer (traits, errors, types, capabilities).
//! - [`engine`] — Core engine: scheduler, transactions, key generation, store/index operations.

#![deny(unsafe_code)]
#![warn(missing_docs)]
#![allow(
    clippy::doc_markdown,
    clippy::struct_field_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

pub mod backend;
pub mod clone;
pub mod engine;
pub mod error;
pub mod key;
pub mod limits;
pub mod proto;
