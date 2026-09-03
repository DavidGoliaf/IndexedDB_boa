//! Job executor for IndexedDB operations.

use boa_engine::Context;

/// Cleans up active transactions at the end of a task.
///
/// This is called after each task to deactivate transactions that
/// are no longer active (§9.3).
pub fn end_of_task(context: &mut Context) {
    crate::runtime::end_of_task(context);
}
