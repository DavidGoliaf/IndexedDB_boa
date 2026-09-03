//! Integration tests for the open-queue FSM (§5.1, §5.3).
//!
//! Covers full upgrade/delete lifecycles; single transitions live in the
//! inline unit tests of `engine::open_queue`.

use boa_idb_core::engine::open_queue::{OpenQueue, OpenQueueAction, OpenQueueState};
use boa_idb_core::error::IdbError;

#[test]
fn upgrade_lifecycle_with_blockers() {
    let mut q = OpenQueue::new("db".into(), 1);
    q.enqueue_open(7, 2);
    q.set_blocking_connections(2);

    // Blocked: must emit SendBlocked and park in WaitingForConnections.
    let action = q.process_next().expect("action");
    assert!(matches!(
        action,
        OpenQueueAction::SendBlocked { request_id: 7 }
    ));
    assert_eq!(q.state(), OpenQueueState::WaitingForConnections);

    // One connection closes: still waiting.
    q.on_connection_closed();
    assert!(q.on_all_connections_closed().is_none());

    // Last one closes: upgrade starts.
    q.on_connection_closed();
    let action = q.on_all_connections_closed().expect("upgrade");
    match action {
        OpenQueueAction::StartUpgrade {
            request_id,
            old_version,
            new_version,
        } => {
            assert_eq!(request_id, 7);
            assert_eq!(old_version, 1);
            assert_eq!(new_version, 2);
        }
        other => panic!("expected StartUpgrade, got {other:?}"),
    }

    q.on_upgrade_complete(2);
    assert_eq!(q.state(), OpenQueueState::Idle);

    // A same-version open now connects directly.
    q.enqueue_open(8, 2);
    let action = q.process_next().expect("open");
    assert!(matches!(
        action,
        OpenQueueAction::OpenConnection {
            request_id: 8,
            version: 2
        }
    ));
}

#[test]
fn old_version_fails_open() {
    let mut q = OpenQueue::new("db".into(), 5);
    q.enqueue_open(1, 3);
    let action = q.process_next().expect("action");
    match action {
        OpenQueueAction::FailOpen { request_id, error } => {
            assert_eq!(request_id, 1);
            assert!(matches!(error, IdbError::Version(_)));
        }
        other => panic!("expected FailOpen, got {other:?}"),
    }
}

#[test]
fn delete_lifecycle_resets_version() {
    let mut q = OpenQueue::new("db".into(), 4);
    q.enqueue_delete(9);
    let action = q.process_next().expect("delete");
    assert!(matches!(
        action,
        OpenQueueAction::StartDelete { request_id: 9 }
    ));
    assert_eq!(q.state(), OpenQueueState::RunningDelete);

    q.on_delete_complete();
    assert_eq!(q.state(), OpenQueueState::Idle);

    // Fresh open after delete sees version 0.
    q.enqueue_open(10, 0);
    let action = q.process_next().expect("open");
    assert!(matches!(
        action,
        OpenQueueAction::OpenConnection {
            request_id: 10,
            version: 0
        }
    ));
}

#[test]
fn failed_operation_returns_to_idle_without_losing_queue() {
    let mut q = OpenQueue::new("db".into(), 1);
    q.enqueue_open(1, 2);
    let action = q.process_next().expect("upgrade");
    assert!(matches!(action, OpenQueueAction::StartUpgrade { .. }));
    q.on_operation_failed();
    assert_eq!(q.state(), OpenQueueState::Idle);
    assert_eq!(q.pending_count(), 0);
}
