//! `IDBTransaction` implementation.

use boa_gc::{Finalize, Trace};

/// Transaction mode.
#[derive(Debug, Clone, PartialEq, Eq, Trace, Finalize)]
pub enum TxnModeJs {
    /// Read-only.
    ReadOnly,
    /// Read-write.
    ReadWrite,
    /// Version change.
    VersionChange,
}

/// Native data for `IDBTransaction`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBTransactionData {
    /// Transaction identifier.
    pub txn_id: u64,
    /// Transaction mode.
    #[unsafe_ignore_trace]
    pub mode: TxnModeJs,
    /// Whether the transaction is active.
    pub active: bool,
}
