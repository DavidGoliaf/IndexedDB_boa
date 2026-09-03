//! `IDBTransaction` implementation.

use boa_engine::JsObject;
use boa_gc::{Finalize, Trace};
use boa_idb_core::proto::TxnMode;

/// Native data for `IDBTransaction`.
#[derive(Debug, Trace, Finalize, boa_engine::JsData)]
pub struct IdBTransactionData {
    /// Transaction identifier.
    #[unsafe_ignore_trace]
    pub txn_id: u64,
    /// Transaction mode.
    #[unsafe_ignore_trace]
    pub mode: TxnMode,
    /// Database reference (GC-traced).
    pub db: JsObject,
    /// Whether the transaction is active.
    pub active: bool,
}
