//! Transaction scheduler with FIFO fairness (§2.7.2).
//!
//! The scheduler manages transaction startup, ensuring:
//! - No conflicting transactions run simultaneously
//! - FIFO ordering is preserved (no starvation of read-write transactions)
//! - VersionChange transactions are exclusive

use crate::proto::{StoreId, TxnId, TxnMode};
use std::collections::VecDeque;

/// A pending transaction request in the scheduler queue.
#[derive(Debug, Clone)]
pub struct TxnQueueItem {
    /// Transaction identifier.
    pub id: TxnId,
    /// Transaction mode.
    pub mode: TxnMode,
    /// Stores accessed by this transaction.
    pub scope: Vec<StoreId>,
}

/// Transaction scheduler implementing FIFO fairness.
#[derive(Debug, Default)]
pub struct TransactionScheduler {
    pending_queue: VecDeque<TxnQueueItem>,
    running_txns: Vec<TxnQueueItem>,
}

impl TransactionScheduler {
    /// Creates a new empty scheduler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues a transaction request.
    pub fn enqueue(&mut self, item: TxnQueueItem) {
        self.pending_queue.push_back(item);
    }

    /// Polls for transactions that can start immediately.
    ///
    /// Returns the IDs of transactions that were started.
    pub fn poll_ready(&mut self) -> Vec<TxnId> {
        let mut ready = Vec::new();
        let mut i = 0;

        while i < self.pending_queue.len() {
            let candidate = &self.pending_queue[i];

            if self.can_start(candidate, i) {
                let item = self.pending_queue.remove(i).unwrap();
                self.running_txns.push(item.clone());
                ready.push(item.id);
            } else {
                // If a readwrite transaction is blocked, we do NOT skip subsequent
                // conflicting transactions to prevent starvation (FIFO fairness).
                i += 1;
            }
        }

        ready
    }

    /// Checks if a candidate transaction can start at the given position in the queue.
    fn can_start(&self, candidate: &TxnQueueItem, index_in_pending: usize) -> bool {
        // 1. Check conflicts with already running transactions
        for running in &self.running_txns {
            if Self::conflicts(candidate, running) {
                return false;
            }
        }

        // 2. Check conflicts with earlier pending transactions
        for earlier in self.pending_queue.iter().take(index_in_pending) {
            if Self::conflicts(candidate, earlier) {
                return false;
            }
        }

        true
    }

    /// Checks if two transactions conflict.
    fn conflicts(a: &TxnQueueItem, b: &TxnQueueItem) -> bool {
        // VersionChange is exclusive with all other transactions
        if a.mode == TxnMode::VersionChange || b.mode == TxnMode::VersionChange {
            return true;
        }
        // Two ReadOnly transactions never conflict
        if a.mode == TxnMode::ReadOnly && b.mode == TxnMode::ReadOnly {
            return false;
        }
        // If either is ReadWrite, check scope intersection
        for s_a in &a.scope {
            if b.scope.contains(s_a) {
                return true;
            }
        }
        false
    }

    /// Notifies the scheduler that a transaction has finished.
    pub fn on_txn_finished(&mut self, id: TxnId) {
        self.running_txns.retain(|t| t.id != id);
    }

    /// Returns the number of pending transactions.
    pub fn pending_count(&self) -> usize {
        self.pending_queue.len()
    }

    /// Returns the number of running transactions.
    pub fn running_count(&self) -> usize {
        self.running_txns.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_readonly_parallel() {
        let mut sched = TransactionScheduler::new();

        // 10 ReadOnly transactions to the same store should all start
        for i in 0..10 {
            sched.enqueue(TxnQueueItem {
                id: i,
                mode: TxnMode::ReadOnly,
                scope: vec![1],
            });
        }

        let ready = sched.poll_ready();
        assert_eq!(ready.len(), 10);
        assert_eq!(sched.running_count(), 10);
    }

    #[test]
    fn test_readwrite_blocks_readonly_same_store() {
        let mut sched = TransactionScheduler::new();

        sched.enqueue(TxnQueueItem {
            id: 1,
            mode: TxnMode::ReadWrite,
            scope: vec![1],
        });
        sched.enqueue(TxnQueueItem {
            id: 2,
            mode: TxnMode::ReadOnly,
            scope: vec![1],
        });

        let ready = sched.poll_ready();
        assert_eq!(ready, vec![1]);
        assert_eq!(sched.running_count(), 1);
        assert_eq!(sched.pending_count(), 1);
    }

    #[test]
    fn test_readwrite_does_not_block_readonly_different_store() {
        let mut sched = TransactionScheduler::new();

        sched.enqueue(TxnQueueItem {
            id: 1,
            mode: TxnMode::ReadWrite,
            scope: vec![1],
        });
        sched.enqueue(TxnQueueItem {
            id: 2,
            mode: TxnMode::ReadOnly,
            scope: vec![2],
        });

        let ready = sched.poll_ready();
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&1));
        assert!(ready.contains(&2));
    }

    #[test]
    fn test_versionchange_exclusive() {
        let mut sched = TransactionScheduler::new();

        sched.enqueue(TxnQueueItem {
            id: 1,
            mode: TxnMode::ReadOnly,
            scope: vec![1],
        });
        sched.enqueue(TxnQueueItem {
            id: 2,
            mode: TxnMode::VersionChange,
            scope: vec![1],
        });

        let ready = sched.poll_ready();
        assert_eq!(ready, vec![1]);

        // VersionChange cannot start while ReadOnly is running
        let ready = sched.poll_ready();
        assert!(ready.is_empty());

        // After ReadOnly finishes, VersionChange can start
        sched.on_txn_finished(1);
        let ready = sched.poll_ready();
        assert_eq!(ready, vec![2]);
    }

    #[test]
    fn test_fifo_no_starvation() {
        let mut sched = TransactionScheduler::new();

        // ReadWrite is first, then many ReadOnly
        sched.enqueue(TxnQueueItem {
            id: 1,
            mode: TxnMode::ReadWrite,
            scope: vec![1],
        });
        for i in 2..12 {
            sched.enqueue(TxnQueueItem {
                id: i,
                mode: TxnMode::ReadOnly,
                scope: vec![1],
            });
        }

        let ready = sched.poll_ready();
        // Only the ReadWrite should start (it's first)
        assert_eq!(ready, vec![1]);

        // ReadOnly transactions should be blocked
        let ready = sched.poll_ready();
        assert!(ready.is_empty());

        // After ReadWrite finishes, all ReadOnly can start
        sched.on_txn_finished(1);
        let ready = sched.poll_ready();
        assert_eq!(ready.len(), 10);
    }

    #[test]
    fn test_txn_finished_removes_from_running() {
        let mut sched = TransactionScheduler::new();

        sched.enqueue(TxnQueueItem {
            id: 1,
            mode: TxnMode::ReadOnly,
            scope: vec![1],
        });
        sched.enqueue(TxnQueueItem {
            id: 2,
            mode: TxnMode::ReadOnly,
            scope: vec![1],
        });

        sched.poll_ready();
        assert_eq!(sched.running_count(), 2);

        sched.on_txn_finished(1);
        assert_eq!(sched.running_count(), 1);
    }
}
