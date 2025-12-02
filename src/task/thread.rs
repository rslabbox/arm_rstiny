use core::time::Duration;

use crate::{hal::percpu, task::task_ops::{task_sleep, task_spawn, task_yield}, task::task_ref::TaskState};

pub struct JoinHandle {
    pub task: super::TaskRef,
}

impl JoinHandle {
    /// Creates a new JoinHandle from a task reference.
    pub(crate) fn new(task: super::TaskRef) -> Self {
        Self { task }
    }

    /// Returns the task ID of the associated task.
    pub fn id(&self) -> super::task_ref::TaskId {
        self.task.id()
    }

    /// Waits for the associated task to finish.
    ///
    /// This function will block the current task until the target task exits.
    ///
    /// # Errors
    ///
    /// Returns `Err(JoinError::SelfJoin)` if attempting to join the current task.
    pub fn join(self) -> crate::TinyResult<()> {
        let curr_task = percpu::current_task();

        // Check for self-join (would deadlock)
        if curr_task.id() == self.task.id() {
            return Err(crate::TinyError::ThreadSelfJoinFailed);
        }

        // Poll until the target task exits
        while self.task.state() != TaskState::Exited {
            task_yield();
        }

        Ok(())
    }
}

/// Spawns a new thread with a function pointer.
///
/// Returns a `JoinHandle` that can be used to wait for the thread to finish.
pub fn spawn(f: fn()) -> JoinHandle {
    task_spawn(f)
}

/// Puts the current thread to sleep for the specified duration.
pub fn sleep(duration: Duration) {
    task_sleep(duration);
}

/// Yields the current thread, allowing other threads to run.
pub fn yield_now() {
    task_yield();
}

/// Returns the current thread's task ID.
pub fn current_id() -> super::task_ref::TaskId {
    percpu::current_task().id()
}
