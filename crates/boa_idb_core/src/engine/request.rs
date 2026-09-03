//! Request queue for transaction operations.

use crate::proto::{OpOutcome, Operation, RequestId};

/// State of a request in the transaction queue.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestState {
    /// Request is pending execution.
    Pending,
    /// Request is currently executing.
    Executing,
    /// Request completed successfully.
    Done(OpOutcome),
    /// Request failed with an error.
    Failed(String),
}

/// A request in the transaction's request queue.
#[derive(Debug, Clone)]
pub struct Request {
    /// Unique request identifier.
    pub id: RequestId,
    /// The operation to perform.
    pub operation: Operation,
    /// Current state of the request.
    pub state: RequestState,
}

impl Request {
    /// Creates a new pending request.
    pub fn new(id: RequestId, operation: Operation) -> Self {
        Self {
            id,
            operation,
            state: RequestState::Pending,
        }
    }
}
