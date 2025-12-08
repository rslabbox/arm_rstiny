use core::{marker::PhantomData, time::Duration};

use crate::{
    hal::percpu,
    task::task_ops::{task_sleep, task_spawn, task_yield},
    task::task_ref::TaskState,
};

/// A handle to a spawned task that can be used to wait for its completion
/// and retrieve its return value.
pub struct JoinHandle<T> {
    pub task: super::TaskRef,
    _marker: PhantomData<T>,
}

impl<T: Send + 'static> JoinHandle<T> {
    /// Creates a new JoinHandle from a task reference.
    pub(crate) fn new(task: super::TaskRef) -> Self {
        Self {
            task,
            _marker: PhantomData,
        }
    }

    /// Returns the task ID of the associated task.
    pub fn id(&self) -> super::task_ref::TaskId {
        self.task.id()
    }

    /// Waits for the associated task to finish and returns its result.
    ///
    /// This function will block the current task until the target task exits.
    ///
    /// # Errors
    ///
    /// Returns `Err(JoinError::SelfJoin)` if attempting to join the current task.
    /// Returns `Err(JoinError::ResultNotAvailable)` if the result cannot be retrieved.
    pub fn join(self) -> crate::TinyResult<T> {
        let curr_task = percpu::current_task();

        // Check for self-join (would deadlock)
        if curr_task.id() == self.task.id() {
            anyhow::bail!("Cannot join thread from itself");
        }

        // Poll until the target task exits
        while self.task.state() != TaskState::Exited {
            task_yield();
        }

        // Retrieve and downcast the result
        if let Some(result) = self.task.take_result() {
            match result.downcast::<T>() {
                Ok(value) => Ok(*value),
                Err(_) => anyhow::bail!("Failed to downcast thread result"),
            }
        } else {
            anyhow::bail!("Thread join failed: result not available")
        }
    }
}

/// Spawns a new thread with a function.
///
/// Returns a `JoinHandle` that can be used to wait for the thread to finish
/// and retrieve its return value.
pub fn spawn<F, T>(name: &'static str, f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    task_spawn(name, f)
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
