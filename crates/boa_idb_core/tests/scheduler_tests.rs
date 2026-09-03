//! Integration tests for the transaction scheduler (FIFO fairness, §2.7.2).
//!
//! These cover multi-step queue scenarios; single-rule cases live in the
//! inline unit tests of `engine::scheduler`.

use boa_idb_core::engine::scheduler::{TransactionScheduler, TxnQueueItem};
use boa_idb_core::proto::TxnMode;

fn item(id: u64, mode: TxnMode, scope: &[u64]) -> TxnQueueItem {
    TxnQueueItem {
        id,
        mode,
        scope: scope.to_vec(),
    }
}

#[test]
fn versionchange_blocks_later_independent_readonly() {
    let mut sched = TransactionScheduler::new();
    // A running ReadOnly on store 1, a pending VersionChange, then an
    // independent ReadOnly on store 2: the latter must NOT jump the queue.
    sched.enqueue(item(1, TxnMode::ReadOnly, &[1]));
    assert_eq!(sched.poll_ready(), vec![1]);

    sched.enqueue(item(2, TxnMode::VersionChange, &[1]));
    sched.enqueue(item(3, TxnMode::ReadOnly, &[2]));
    assert!(sched.poll_ready().is_empty());

    sched.on_txn_finished(1);
    // VersionChange starts; the independent ReadOnly still waits.
    assert_eq!(sched.poll_ready(), vec![2]);

    sched.on_txn_finished(2);
    assert_eq!(sched.poll_ready(), vec![3]);
}

#[test]
fn readwrite_chain_preserves_fifo() {
    let mut sched = TransactionScheduler::new();
    sched.enqueue(item(1, TxnMode::ReadWrite, &[1]));
    sched.enqueue(item(2, TxnMode::ReadWrite, &[1]));
    sched.enqueue(item(3, TxnMode::ReadOnly, &[2]));

    // 1 starts; 2 conflicts with running 1; 3 is independent and may start.
    let ready = sched.poll_ready();
    assert!(ready.contains(&1));
    assert!(ready.contains(&3));
    assert!(!ready.contains(&2));

    sched.on_txn_finished(1);
    assert_eq!(sched.poll_ready(), vec![2]);
}

#[test]
fn independent_readwrite_stores_run_parallel() {
    let mut sched = TransactionScheduler::new();
    sched.enqueue(item(1, TxnMode::ReadWrite, &[1]));
    sched.enqueue(item(2, TxnMode::ReadWrite, &[2]));
    let ready = sched.poll_ready();
    assert_eq!(ready.len(), 2);
}
