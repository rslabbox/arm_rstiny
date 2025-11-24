//! High-level thread API for task creation and control.

use alloc::boxed::Box;
use core::time::Duration;

use crate::drivers::timer::busy_wait;

use super::task::{TaskId, TaskState};

/// A handle to a spawned task.
///
/// This handle can be used to wait for the task to complete via `join()`.
pub struct JoinHandle {
    task_id: TaskId,
}

impl JoinHandle {
    /// Create a new join handle for a task.
    fn new(task_id: TaskId) -> Self {
        Self { task_id }
    }

    /// Wait for the task to complete.
    ///
    /// This function blocks until the task finishes execution.
    pub fn join(self) {
        loop {
            // Check if task is dead
            let guard = super::scheduler_lock();
            if let Some(scheduler) = guard.as_ref() {
                if let Some(task) = scheduler.get_task(self.task_id) {
                    if task.state == TaskState::Dead {
                        debug!("Task {} completed", self.task_id);
                        return;
                    }
                } else {
                    // Task not found (shouldn't happen)
                    warn!("Task {} not found", self.task_id);
                    return;
                }
            }
            drop(guard);

            // Yield to let other tasks run
            busy_wait(Duration::from_micros(100));
        }
    }

    /// Get the task ID.
    pub fn id(&self) -> TaskId {
        self.task_id
    }
}

/// Spawn a new task.
///
/// # Arguments
/// * `f` - Closure to execute in the new task
///
/// # Returns
/// A `JoinHandle` that can be used to wait for the task to complete.
///
/// # Examples
/// ```no_run
/// let handle = thread::spawn(|| {
///     println!("Hello from task!");
/// });
/// handle.join(); // Wait for task to complete
/// ```
pub fn spawn<F>(f: F) -> JoinHandle
where
    F: FnOnce() + Send + 'static,
{
    // Box the closure on the heap
    let boxed = Box::new(f);
    let raw = Box::into_raw(boxed);

    // Get scheduler and spawn task
    let mut guard = super::scheduler_lock();
    let scheduler = guard.as_mut().expect("Scheduler not initialized");
    
    let next_tid = scheduler.next_task_id();
    let tid = scheduler.spawn(
        alloc::format!("task-{}", next_tid),
        task_entry::<F> as usize,
        raw as usize,
    );

    JoinHandle::new(tid)
}

/// Yield the CPU to another task.
///
/// The current task will be moved to the back of the ready queue
/// and another task will be scheduled.
///
/// # Examples
/// ```no_run
/// loop {
///     // Do some work
///     thread::yield_now();
/// }
/// ```
pub fn yield_now() {
    // This is a simplified implementation
    // In a real system, this would trigger a syscall or software interrupt
    // For now, we'll just wait for the next timer interrupt to trigger scheduling
    warn!("yield_now: explicit yield not fully implemented yet, waiting for timer");
}

/// Sleep for the specified duration.
///
/// The current task will be blocked and woken up after the duration expires.
///
/// # Arguments
/// * `duration` - How long to sleep
///
/// # Examples
/// ```no_run
/// use core::time::Duration;
/// thread::sleep(Duration::from_millis(100));
/// ```
pub fn sleep(duration: Duration) {
    // This is a simplified implementation
    // In a real system, this would trigger a syscall
    // For now, we store the request and let the next interrupt handle it
    
    let duration_ns = duration.as_nanos() as u64;
    debug!("Task requesting sleep for {} ns", duration_ns);
    
    // Store sleep request in thread-local or global state
    // The actual sleep will be handled on the next timer interrupt
    super::set_sleep_request(duration_ns);
}

/// Get the current task ID.
///
/// # Returns
/// The current task's ID, or None if not in a task context.
pub fn current() -> Option<TaskId> {
    let guard = super::scheduler_lock();
    let scheduler = guard.as_ref()?;
    scheduler.current_id()
}

/// Task entry point wrapper.
///
/// This function is called when a task starts. It executes the user's closure
/// and marks the task as dead when it returns.
///
/// # Arguments
/// * `arg` - Raw pointer to the boxed closure
fn task_entry<F>(arg: usize) -> !
where
    F: FnOnce() + Send + 'static,
{
    // Reconstruct the box from the raw pointer
    let boxed = unsafe { Box::from_raw(arg as *mut F) };

    // Execute the task
    boxed();

    // Task finished - mark as dead and trigger scheduling
    debug!("Task finished, marking as dead");
    task_exit();
}

/// Mark the current task as dead and schedule another task.
///
/// This should be called when a task finishes execution.
fn task_exit() -> ! {
    // Mark current task as Dead in the scheduler
    let mut guard = super::scheduler_lock();
    if let Some(scheduler) = guard.as_mut() {
        scheduler.exit_current();
    }
    drop(guard);
    
    // Wait for next interrupt to switch to another task
    // Use WFI (Wait For Interrupt) to save power
    loop {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfi");
        }
        #[cfg(not(target_arch = "aarch64"))]
        core::hint::spin_loop();
    }
}
