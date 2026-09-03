//! Backend capabilities description.

/// Describes the capabilities of a storage backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Whether the backend supports snapshot isolation for read-only transactions.
    pub snapshot_isolation: bool,
    /// Whether the backend supports durable storage (survives restarts).
    pub durable: bool,
    /// Whether the backend supports concurrent transactions.
    pub concurrent: bool,
    /// Maximum key size in bytes (None = unlimited).
    pub max_key_size: Option<usize>,
    /// Maximum value size in bytes (None = unlimited).
    pub max_value_size: Option<usize>,
}

impl Default for BackendCapabilities {
    fn default() -> Self {
        Self {
            snapshot_isolation: true,
            durable: false,
            concurrent: true,
            max_key_size: None,
            max_value_size: None,
        }
    }
}
