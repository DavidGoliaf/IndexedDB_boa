//! In-memory cursor implementation.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendCursor;
use boa_idb_core::backend::types::CursorSeek;

/// In-memory cursor (stub implementation).
#[derive(Debug)]
pub struct MemoryCursor {
    current_key: Vec<u8>,
    current_primary_key: Vec<u8>,
    current_value: Option<Vec<u8>>,
    exhausted: bool,
}

impl MemoryCursor {
    /// Creates a new empty memory cursor.
    pub fn new() -> Self {
        Self {
            current_key: Vec::new(),
            current_primary_key: Vec::new(),
            current_value: None,
            exhausted: true,
        }
    }
}

impl Default for MemoryCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendCursor for MemoryCursor {
    fn seek(&mut self, _target: CursorSeek) -> Result<bool, BackendError> {
        // TODO: implement seek
        self.exhausted = true;
        Ok(false)
    }

    fn step(&mut self, _count: u32) -> Result<bool, BackendError> {
        // TODO: implement step
        self.exhausted = true;
        Ok(false)
    }

    fn current_key(&self) -> &[u8] {
        &self.current_key
    }

    fn current_primary_key(&self) -> &[u8] {
        &self.current_primary_key
    }

    fn current_value(&self) -> Option<&[u8]> {
        self.current_value.as_deref()
    }
}
