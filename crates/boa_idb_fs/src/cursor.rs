//! Pre-collected cursor over merged FS backend entries.

use boa_idb_core::backend::error::BackendError;
use boa_idb_core::backend::traits::BackendCursor;
use boa_idb_core::backend::types::CursorSeek;

type CursorEntry = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);

/// Cursor that iterates a snapshot of entries collected at `scan` time.
pub struct FsCursor {
    entries: Vec<CursorEntry>,
    position: usize,
    exhausted: bool,
}

impl FsCursor {
    pub(crate) fn with_entries(entries: Vec<CursorEntry>) -> Self {
        let exhausted = entries.is_empty();
        Self {
            entries,
            position: 0,
            exhausted,
        }
    }

    pub(crate) fn with_entries_reversed(mut entries: Vec<CursorEntry>) -> Self {
        entries.reverse();
        Self::with_entries(entries)
    }
}

impl BackendCursor for FsCursor {
    fn seek(&mut self, target: CursorSeek) -> Result<bool, BackendError> {
        match target {
            CursorSeek::First => {
                self.position = 0;
                self.exhausted = self.entries.is_empty();
                Ok(!self.exhausted)
            }
            CursorSeek::Key(key) => {
                self.position = self
                    .entries
                    .iter()
                    .position(|(k, _, _)| k.as_slice() >= key.as_slice())
                    .unwrap_or(self.entries.len());
                self.exhausted = self.position >= self.entries.len();
                Ok(!self.exhausted)
            }
            CursorSeek::KeyAndPrimaryKey { key, pkey } => {
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
            &[]
        } else {
            &self.entries[self.position].0
        }
    }

    fn current_primary_key(&self) -> &[u8] {
        if self.exhausted || self.position >= self.entries.len() {
            &[]
        } else {
            &self.entries[self.position].1
        }
    }

    fn current_value(&self) -> Option<&[u8]> {
        if self.exhausted || self.position >= self.entries.len() {
            None
        } else {
            self.entries[self.position].2.as_deref()
        }
    }
}
