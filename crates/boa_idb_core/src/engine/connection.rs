//! Database connection management.

use crate::proto::ConnectionId;

/// State of a database connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Connection is being opened (waiting for version check).
    Opening,
    /// Connection is open and ready.
    Open,
    /// Connection is closing.
    Closing,
    /// Connection is closed.
    Closed,
}

/// A database connection (host-side representation of IDBDatabase).
#[derive(Debug)]
pub struct Connection {
    /// Unique connection identifier.
    pub id: ConnectionId,
    /// Database name.
    pub database_name: String,
    /// Current state.
    state: ConnectionState,
}

impl Connection {
    /// Creates a new connection in the Opening state.
    pub fn new(id: ConnectionId, database_name: String) -> Self {
        Self {
            id,
            database_name,
            state: ConnectionState::Opening,
        }
    }

    /// Returns the current state.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Transitions to the Open state.
    pub fn set_open(&mut self) {
        self.state = ConnectionState::Open;
    }

    /// Transitions to the Closing state.
    pub fn start_close(&mut self) {
        self.state = ConnectionState::Closing;
    }

    /// Transitions to the Closed state.
    pub fn finish_close(&mut self) {
        self.state = ConnectionState::Closed;
    }

    /// Checks if the connection is open.
    pub fn is_open(&self) -> bool {
        self.state == ConnectionState::Open
    }

    /// Checks if the connection is closed.
    pub fn is_closed(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Closing | ConnectionState::Closed
        )
    }
}
