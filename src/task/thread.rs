//! User-friendly thread API.
//!
//! This module provides a familiar thread-like interface for task management.

use core::time::Duration;

use crate::hal::percpu;

use super::manager;
use super::task::TaskId;

/// Spawns a new thread with a function pointer.
///
/// For simplicity in kernel context, we use function pointers directly.
pub fn spawn(f: fn()) -> TaskId {
    manager::spawn("thread", f)
}

/// Puts the current thread to sleep for the specified duration.
pub fn sleep(duration: Duration) {
    let nanos = duration.as_nanos() as u64;
    manager::sleep(nanos);
}

/// Yields the current thread, allowing other threads to run.
pub fn yield_now() {
    manager::yield_now();
}

/// Returns the current thread's task ID.
pub fn current_id() -> TaskId {
    percpu::current_task().id()
}
