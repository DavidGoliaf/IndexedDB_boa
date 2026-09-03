//! Error types for storage backends.

use thiserror::Error;

/// Errors that can occur in storage backend operations.
#[derive(Debug, Error)]
pub enum BackendError {
    /// Constraint violation (e.g., unique index, duplicate key).
    #[error("Constraint violation: {0}")]
    Constraint(String),

    /// Entity not found.
    #[error("Entity not found: {0}")]
    NotFound(String),

    /// Invalid key range.
    #[error("Invalid key range")]
    InvalidRange,

    /// Database is locked by another process or transaction.
    #[error("Database is locked by another process or transaction")]
    Locked,

    /// Data corruption detected.
    #[error("Data corruption: {0}")]
    Corrupted(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(String),

    /// Storage quota exceeded.
    #[error("Storage quota exceeded: required {needed} bytes, available {available} bytes")]
    QuotaExceeded {
        /// Bytes needed.
        needed: u64,
        /// Bytes available.
        available: u64,
    },

    /// Internal backend error.
    #[error("Internal backend error: {0}")]
    Internal(String),
}
