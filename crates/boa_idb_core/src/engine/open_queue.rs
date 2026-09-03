//! FSM for database open/delete queue (§5.1, §5.3).
//!
//! Manages the FIFO queue of open/delete requests for a single database,
//! handling version comparison, blocked events, and versionchange transactions.

use crate::error::IdbError;
use std::collections::VecDeque;

/// Type of request in the open queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRequestType {
    /// Opening a database connection.
    Open {
        /// Requested version (0 = any version).
        requested_version: u64,
    },
    /// Deleting a database.
    Delete,
}

/// A pending open/delete request.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    /// Type of request.
    pub request_type: OpenRequestType,
    /// Unique request identifier.
    pub request_id: u64,
}

/// State of the open queue FSM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenQueueState {
    /// Idle, no pending requests.
    Idle,
    /// Waiting for existing connections to close.
    WaitingForConnections,
    /// Running a version change transaction.
    RunningUpgrade,
    /// Running a delete operation.
    RunningDelete,
}

/// Queue for managing open/delete requests for a single database.
#[derive(Debug)]
pub struct OpenQueue {
    /// Database name.
    pub database_name: String,
    /// Current database version (0 if not yet created).
    pub current_version: u64,
    /// Queue of pending requests.
    queue: VecDeque<OpenRequest>,
    /// Current state of the FSM.
    state: OpenQueueState,
    /// Number of active connections blocking the operation.
    blocking_connections: u32,
}

impl OpenQueue {
    /// Creates a new open queue for a database.
    pub fn new(database_name: String, current_version: u64) -> Self {
        Self {
            database_name,
            current_version,
            queue: VecDeque::new(),
            state: OpenQueueState::Idle,
            blocking_connections: 0,
        }
    }

    /// Enqueues an open request.
    pub fn enqueue_open(&mut self, request_id: u64, requested_version: u64) {
        self.queue.push_back(OpenRequest {
            request_type: OpenRequestType::Open { requested_version },
            request_id,
        });
    }

    /// Enqueues a delete request.
    pub fn enqueue_delete(&mut self, request_id: u64) {
        self.queue.push_back(OpenRequest {
            request_type: OpenRequestType::Delete,
            request_id,
        });
    }

    /// Returns the current state.
    pub fn state(&self) -> OpenQueueState {
        self.state
    }

    /// Sets the number of blocking connections.
    pub fn set_blocking_connections(&mut self, count: u32) {
        self.blocking_connections = count;
    }

    /// Decrements the blocking connection count (called when a connection closes).
    pub fn on_connection_closed(&mut self) {
        self.blocking_connections = self.blocking_connections.saturating_sub(1);
    }

    /// Returns the number of blocking connections.
    pub fn blocking_connections(&self) -> u32 {
        self.blocking_connections
    }

    /// Processes the next request in the queue.
    ///
    /// Returns the action to take, or None if the queue is empty or blocked.
    pub fn process_next(&mut self) -> Option<OpenQueueAction> {
        if self.state != OpenQueueState::Idle {
            return None;
        }

        // Clone the front request data to avoid borrow conflicts
        let request = self.queue.front()?.clone();
        let request_id = request.request_id;

        match &request.request_type {
            OpenRequestType::Open { requested_version } => {
                if *requested_version == 0 {
                    // Opening any version - just open current
                    self.queue.pop_front();
                    Some(OpenQueueAction::OpenConnection {
                        request_id,
                        version: self.current_version,
                    })
                } else if *requested_version < self.current_version {
                    // Requested version is too old
                    self.queue.pop_front();
                    Some(OpenQueueAction::FailOpen {
                        request_id,
                        error: IdbError::Version(format!(
                            "Requested version {} is less than current version {}",
                            requested_version, self.current_version
                        )),
                    })
                } else if *requested_version == self.current_version {
                    // Same version - just open
                    self.queue.pop_front();
                    Some(OpenQueueAction::OpenConnection {
                        request_id,
                        version: self.current_version,
                    })
                } else {
                    // Need to upgrade
                    if self.blocking_connections > 0 {
                        self.state = OpenQueueState::WaitingForConnections;
                        Some(OpenQueueAction::SendBlocked { request_id })
                    } else {
                        self.state = OpenQueueState::RunningUpgrade;
                        self.queue.pop_front();
                        Some(OpenQueueAction::StartUpgrade {
                            request_id,
                            old_version: self.current_version,
                            new_version: *requested_version,
                        })
                    }
                }
            }
            OpenRequestType::Delete => {
                if self.blocking_connections > 0 {
                    self.state = OpenQueueState::WaitingForConnections;
                    Some(OpenQueueAction::SendBlocked { request_id })
                } else {
                    self.state = OpenQueueState::RunningDelete;
                    self.queue.pop_front();
                    Some(OpenQueueAction::StartDelete { request_id })
                }
            }
        }
    }

    /// Called when all blocking connections have closed.
    pub fn on_all_connections_closed(&mut self) -> Option<OpenQueueAction> {
        if self.state != OpenQueueState::WaitingForConnections {
            return None;
        }

        // Only proceed if there are actually no more blocking connections
        if self.blocking_connections > 0 {
            return None;
        }

        self.state = OpenQueueState::Idle;
        self.process_next()
    }

    /// Called when an upgrade completes successfully.
    pub fn on_upgrade_complete(&mut self, new_version: u64) {
        self.current_version = new_version;
        self.state = OpenQueueState::Idle;
    }

    /// Called when a delete completes successfully.
    pub fn on_delete_complete(&mut self) {
        self.current_version = 0;
        self.state = OpenQueueState::Idle;
    }

    /// Called when an upgrade or delete fails.
    pub fn on_operation_failed(&mut self) {
        self.state = OpenQueueState::Idle;
    }

    /// Returns the number of pending requests.
    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }
}

/// Actions that the open queue can request.
#[derive(Debug)]
pub enum OpenQueueAction {
    /// Open a connection with the given version.
    OpenConnection {
        /// Request ID.
        request_id: u64,
        /// Database version.
        version: u64,
    },
    /// Fail an open request with an error.
    FailOpen {
        /// Request ID.
        request_id: u64,
        /// The error.
        error: IdbError,
    },
    /// Send a blocked event (waiting for connections to close).
    SendBlocked {
        /// Request ID.
        request_id: u64,
    },
    /// Start a version upgrade transaction.
    StartUpgrade {
        /// Request ID.
        request_id: u64,
        /// Old database version.
        old_version: u64,
        /// New database version.
        new_version: u64,
    },
    /// Start a database delete operation.
    StartDelete {
        /// Request ID.
        request_id: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_same_version() {
        let mut queue = OpenQueue::new("test".into(), 1);
        queue.enqueue_open(1, 1);

        let action = queue.process_next();
        assert!(matches!(
            action,
            Some(OpenQueueAction::OpenConnection {
                request_id: 1,
                version: 1
            })
        ));
    }

    #[test]
    fn test_open_any_version() {
        let mut queue = OpenQueue::new("test".into(), 5);
        queue.enqueue_open(1, 0);

        let action = queue.process_next();
        assert!(matches!(
            action,
            Some(OpenQueueAction::OpenConnection {
                request_id: 1,
                version: 5
            })
        ));
    }

    #[test]
    fn test_open_old_version_fails() {
        let mut queue = OpenQueue::new("test".into(), 5);
        queue.enqueue_open(1, 3);

        let action = queue.process_next();
        assert!(matches!(action, Some(OpenQueueAction::FailOpen { .. })));
    }

    #[test]
    fn test_upgrade_blocked() {
        let mut queue = OpenQueue::new("test".into(), 1);
        queue.set_blocking_connections(1);
        queue.enqueue_open(1, 2);

        let action = queue.process_next();
        assert!(matches!(action, Some(OpenQueueAction::SendBlocked { .. })));
        assert_eq!(queue.state(), OpenQueueState::WaitingForConnections);
    }

    #[test]
    fn test_upgrade_starts_when_no_connections() {
        let mut queue = OpenQueue::new("test".into(), 1);
        queue.enqueue_open(1, 2);

        let action = queue.process_next();
        assert!(matches!(
            action,
            Some(OpenQueueAction::StartUpgrade {
                request_id: 1,
                old_version: 1,
                new_version: 2
            })
        ));
    }

    #[test]
    fn test_delete_blocked() {
        let mut queue = OpenQueue::new("test".into(), 1);
        queue.set_blocking_connections(1);
        queue.enqueue_delete(1);

        let action = queue.process_next();
        assert!(matches!(action, Some(OpenQueueAction::SendBlocked { .. })));
    }

    #[test]
    fn test_connection_close_unblocks() {
        let mut queue = OpenQueue::new("test".into(), 1);
        queue.set_blocking_connections(2);
        queue.enqueue_open(1, 2);

        queue.process_next(); // Returns SendBlocked
        assert_eq!(queue.state(), OpenQueueState::WaitingForConnections);

        queue.on_connection_closed();
        // Still waiting (1 connection left)
        let action = queue.on_all_connections_closed();
        assert!(action.is_none());

        queue.on_connection_closed();
        // Now all connections closed
        let action = queue.on_all_connections_closed();
        assert!(matches!(action, Some(OpenQueueAction::StartUpgrade { .. })));
    }

    #[test]
    fn test_upgrade_complete() {
        let mut queue = OpenQueue::new("test".into(), 1);
        queue.enqueue_open(1, 2);

        queue.process_next(); // StartUpgrade
        queue.on_upgrade_complete(2);

        assert_eq!(queue.current_version, 2);
        assert_eq!(queue.state(), OpenQueueState::Idle);
    }

    #[test]
    fn test_fifo_ordering() {
        let mut queue = OpenQueue::new("test".into(), 1);
        queue.enqueue_open(1, 0);
        queue.enqueue_open(2, 0);

        let action = queue.process_next();
        assert!(matches!(
            action,
            Some(OpenQueueAction::OpenConnection { request_id: 1, .. })
        ));

        let action = queue.process_next();
        assert!(matches!(
            action,
            Some(OpenQueueAction::OpenConnection { request_id: 2, .. })
        ));
    }
}
