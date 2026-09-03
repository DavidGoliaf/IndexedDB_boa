//! Transaction state management and auto-commit FSM.

use crate::proto::{Durability, StoreId, TxnId, TxnMode};

/// State of an IDB transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnState {
    /// Transaction is active and can accept requests.
    Active,
    /// Transaction is committing.
    Committing,
    /// Transaction has been committed.
    Committed,
    /// Transaction is aborting.
    Aborting,
    /// Transaction has been aborted.
    Aborted,
    /// Transaction is inactive (between requests).
    Inactive,
}

/// An IDB transaction with its metadata and state.
#[derive(Debug)]
pub struct Transaction {
    /// Transaction identifier.
    pub id: TxnId,
    /// Transaction mode.
    pub mode: TxnMode,
    /// Stores in the transaction scope.
    pub scope: Vec<StoreId>,
    /// Durability guarantee.
    pub durability: Durability,
    /// Current state.
    state: TxnState,
}

impl Transaction {
    /// Creates a new transaction.
    pub fn new(id: TxnId, mode: TxnMode, scope: Vec<StoreId>, durability: Durability) -> Self {
        Self {
            id,
            mode,
            scope,
            durability,
            state: TxnState::Active,
        }
    }

    /// Returns the current state.
    pub fn state(&self) -> TxnState {
        self.state
    }

    /// Checks if the transaction is active.
    pub fn is_active(&self) -> bool {
        self.state == TxnState::Active
    }

    /// Checks if the transaction is finished (committed or aborted).
    pub fn is_finished(&self) -> bool {
        matches!(self.state, TxnState::Committed | TxnState::Aborted)
    }

    /// Transitions to the Committing state.
    pub fn start_commit(&mut self) -> bool {
        if self.state == TxnState::Active {
            self.state = TxnState::Committing;
            true
        } else {
            false
        }
    }

    /// Transitions to the Committed state.
    pub fn finish_commit(&mut self) {
        self.state = TxnState::Committed;
    }

    /// Transitions to the Aborting state.
    pub fn start_abort(&mut self) -> bool {
        if self.state != TxnState::Committed && self.state != TxnState::Aborted {
            self.state = TxnState::Aborting;
            true
        } else {
            false
        }
    }

    /// Transitions to the Aborted state.
    pub fn finish_abort(&mut self) {
        self.state = TxnState::Aborted;
    }

    /// Checks if the transaction scope includes the given store.
    pub fn includes_store(&self, store: StoreId) -> bool {
        self.scope.contains(&store)
    }
}
