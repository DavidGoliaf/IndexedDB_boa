//! Core cursor implementation and iteration algorithm (§6.7).

use crate::backend::traits::BackendCursor;
use crate::backend::types::CursorSeek;
use crate::clone::decode::decode_scf;
use crate::clone::scvalue::ScValue;
use crate::error::IdbError;
use crate::key::encode::decode_key;
use crate::key::value::Key;
use crate::limits::LimitConfig;
use crate::proto::{Direction, SourceRef};

/// State of a core cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorState {
    /// Cursor is at a valid position.
    Active,
    /// Cursor has reached the end.
    Exhausted,
    /// Cursor has been closed.
    Closed,
}

/// A core cursor wrapping a backend cursor with key/value decoding.
pub struct CoreCursor<'a> {
    /// Cursor identifier.
    pub id: u64,
    /// Source (store or index).
    pub source: SourceRef,
    /// Direction.
    pub direction: Direction,
    /// Whether this is a key-only cursor.
    pub key_only: bool,
    /// Current state.
    state: CursorState,
    /// Backend cursor.
    backend: Box<dyn BackendCursor + 'a>,
    /// Limits configuration.
    limits: &'a LimitConfig,
}

impl<'a> CoreCursor<'a> {
    /// Creates a new core cursor.
    pub fn new(
        id: u64,
        source: SourceRef,
        direction: Direction,
        key_only: bool,
        backend: Box<dyn BackendCursor + 'a>,
        limits: &'a LimitConfig,
    ) -> Self {
        Self {
            id,
            source,
            direction,
            key_only,
            state: CursorState::Active,
            backend,
            limits,
        }
    }

    /// Returns the current state.
    pub fn state(&self) -> CursorState {
        self.state
    }

    /// Seeks the cursor to the given target.
    pub fn seek(&mut self, target: CursorSeek) -> Result<bool, IdbError> {
        let found = self
            .backend
            .seek(target)
            .map_err(|e| IdbError::Data(format!("Cursor seek failed: {e}")))?;
        if !found {
            self.state = CursorState::Exhausted;
        }
        Ok(found)
    }

    /// Advances the cursor by the given count.
    pub fn advance(&mut self, count: u32) -> Result<bool, IdbError> {
        if self.state != CursorState::Active {
            return Ok(false);
        }
        let found = self
            .backend
            .step(count)
            .map_err(|e| IdbError::Data(format!("Cursor step failed: {e}")))?;
        if !found {
            self.state = CursorState::Exhausted;
        }
        Ok(found)
    }

    /// Returns the current key (decoded).
    pub fn current_key(&self) -> Result<Option<Key>, IdbError> {
        if self.state != CursorState::Active {
            return Ok(None);
        }
        let bytes = self.backend.current_key();
        if bytes.is_empty() {
            return Ok(None);
        }
        let (key, _) = decode_key(bytes)
            .map_err(|e| IdbError::Data(format!("Failed to decode cursor key: {e}")))?;
        Ok(Some(key))
    }

    /// Returns the current primary key (decoded).
    pub fn current_primary_key(&self) -> Result<Option<Key>, IdbError> {
        if self.state != CursorState::Active {
            return Ok(None);
        }
        let bytes = self.backend.current_primary_key();
        if bytes.is_empty() {
            return Ok(None);
        }
        let (key, _) = decode_key(bytes)
            .map_err(|e| IdbError::Data(format!("Failed to decode cursor primary key: {e}")))?;
        Ok(Some(key))
    }

    /// Returns the current value (decoded).
    pub fn current_value(&self) -> Result<Option<ScValue>, IdbError> {
        if self.state != CursorState::Active || self.key_only {
            return Ok(None);
        }
        match self.backend.current_value() {
            Some(bytes) => {
                let value = decode_scf(bytes, self.limits)
                    .map_err(|e| IdbError::Data(format!("Failed to decode cursor value: {e}")))?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Closes the cursor.
    pub fn close(&mut self) {
        self.state = CursorState::Closed;
    }
}
