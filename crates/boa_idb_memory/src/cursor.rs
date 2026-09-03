//! In-memory cursor implementation.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendCursor;
use boa_idb_core::backend::types::CursorSeek;

/// Type alias for cursor entries: (key, primary_key, optional_value).
type CursorEntry = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);

/// In-memory cursor that iterates over a pre-collected set of records.
pub struct MemoryCursor {
    entries: Vec<CursorEntry>,
    position: usize,
    exhausted: bool,
}

impl MemoryCursor {
    /// Creates a new empty memory cursor.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            position: 0,
            exhausted: true,
        }
    }

    /// Creates a memory cursor with pre-collected entries.
    #[allow(dead_code)]
    pub fn with_entries(entries: Vec<CursorEntry>) -> Self {
        let exhausted = entries.is_empty();
        Self {
            entries,
            position: 0,
            exhausted,
        }
    }

    /// Creates a memory cursor with entries sorted in reverse order.
    #[allow(dead_code)]
    pub fn with_entries_reversed(mut entries: Vec<CursorEntry>) -> Self {
        entries.reverse();
        let exhausted = entries.is_empty();
        Self {
            entries,
            position: 0,
            exhausted,
        }
    }
}

impl Default for MemoryCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl BackendCursor for MemoryCursor {
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError> {
        match target {
            CursorSeek::First => {
                self.position = 0;
                self.exhausted = self.entries.is_empty();
                Ok(!self.exhausted)
            }
            CursorSeek::Key(key) => {
                // Find the first entry with key >= target
                self.position = self
                    .entries
                    .iter()
                    .position(|(k, _, _)| k.as_slice() >= key.as_slice())
                    .unwrap_or(self.entries.len());
                self.exhausted = self.position >= self.entries.len();
                Ok(!self.exhausted)
            }
            CursorSeek::KeyAndPrimaryKey { key, pkey } => {
                // Find the first entry with (key, pkey) >= target
                self.position = self
                    .entries
                    .iter()
                    .position(|(k, pk, _)| {
                        (k.as_slice(), pk.as_slice()) >= (key.as_slice(), pkey.as_slice())
                    })
                    .unwrap_or(self.entries.len());
                self.exhausted = self.position >= self.entries.len();
                Ok(!self.exhausted)
            }
        }
    }

    fn step(&mut self, count: u32) -> Result<bool, BackendError> {
        self.position += count as usize;
        self.exhausted = self.position >= self.entries.len();
        Ok(!self.exhausted)
    }

    fn current_key(&self) -> &[u8] {
        if self.exhausted || self.position >= self.entries.len() {
            return &[];
        }
        &self.entries[self.position].0
    }

    fn current_primary_key(&self) -> &[u8] {
        if self.exhausted || self.position >= self.entries.len() {
            return &[];
        }
        &self.entries[self.position].1
    }

    fn current_value(&self) -> Option<&[u8]> {
        if self.exhausted || self.position >= self.entries.len() {
            return None;
        }
        self.entries[self.position].2.as_deref()
    }
}
