//! Security limits and quotas configuration.

/// Safety limits and quotas for the core engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitConfig {
    /// Maximum encoded key size in bytes (default 1024).
    pub max_key_len: usize,
    /// Maximum nesting depth for key arrays (default 32).
    pub max_key_depth: usize,
    /// Maximum serialized value size in bytes (default 64 MiB).
    pub max_value_len: usize,
    /// Maximum nesting depth when cloning values (default 512).
    pub max_clone_depth: usize,
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            max_key_len: 1024,
            max_key_depth: 32,
            max_value_len: 64 * 1024 * 1024,
            max_clone_depth: 512,
        }
    }
}
