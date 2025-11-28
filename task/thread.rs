//! User-friendly thread API.
//!
//! This module provides a familiar thread-like interface for task management.

use core::time::Duration;

use crate::{TinyError, TinyResult};
use crate::hal::percpu;

use super::manager;
use super::task::{TaskId, TaskRef};

/// A handle to a spawned task that can be used to wait for its completion.
pub struct JoinHandle {
    task: TaskRef,
}

impl JoinHandle {
    /// Creates a new JoinHandle from a task reference.
    pub(crate) fn new(task: TaskRef) -> Self {
        Self { task }
    }

    /// Returns the task ID of the associated task.
    pub fn id(&self) -> TaskId {
        self.task.id()
    }

    /// Waits for the associated task to finish.
    ///
    /// This function will block the current task until the target task exits.
    ///
    /// # Errors
    ///
    /// Returns `Err(JoinError::SelfJoin)` if attempting to join the current task.
    pub fn join(self) -> TinyResult<()> {
        let curr_task = percpu::current_task();

        // Check for self-join (would deadlock)
        if curr_task.id() == self.task.id() {
            return Err(TinyError::ThreadSelfJoinFailed);
        }

        manager::join(self.task);
        Ok(())
    }
}

/// Spawns a new thread with a function pointer.
///
/// Returns a `JoinHandle` that can be used to wait for the thread to finish.
pub fn spawn(f: fn()) -> JoinHandle {
    let task = manager::spawn("thread", f);
    JoinHandle::new(task)
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
