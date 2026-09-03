//! IO runtime and channel synchronization.

/// Placeholder for IO runtime that bridges JS thread with storage backend.
pub struct IoRuntime;

impl IoRuntime {
    /// Creates a new IO runtime.
    pub fn new() -> Self {
        Self
    }
}

impl Default for IoRuntime {
    fn default() -> Self {
        Self::new()
    }
}
