//! Thread API for task management.

use alloc::boxed::Box;
use core::time::Duration;

use super::task::TaskId;

/// Spawn a new task with a closure.
///
/// # Example
/// ```
/// let task_id = thread::spawn(|| {
///     println!("Hello from task!");
/// });
/// ```
pub fn spawn<F>(f: F) -> TaskId
where
    F: FnOnce() + Send + 'static,
{
    // Box the closure on the heap
    let closure = Box::new(f);
    let closure_ptr = Box::into_raw(closure);
    
    // Get task manager and spawn task
    super::with_task_manager(|tm| {
        let parent_id = tm.current_task;
        tm.spawn_with_parent(
            alloc::format!("task-{}", tm.current_task.map(|id| id.as_usize()).unwrap_or(0)),
            task_trampoline::<F> as usize,
            closure_ptr as usize,
            parent_id,
        )
    })
}

/// Put current task to sleep for the specified duration.
///
/// This will yield the CPU to other tasks.
///
/// # Example
/// ```
/// thread::sleep(Duration::from_millis(100));
/// ```
pub fn sleep(duration: Duration) {
    let task_id = current_task_id().expect("Cannot sleep without current task");
    
    // Register timer callback to wake up this task
    crate::drivers::timer::set_timer(duration, move |_now| {
        crate::task::wake_task(task_id);
    });
    
    // Mark current task as sleeping
    super::with_task_manager(|tm| {
        tm.mark_sleeping(task_id);
    });
    
    // Wait for interrupt to wake us up
    unsafe {
        core::arch::asm!("wfi");
    }
}

/// Get the current task ID.
///
/// Returns `None` if called before scheduler initialization.
#[allow(dead_code)]
pub fn current_task_id() -> Option<TaskId> {
    super::with_task_manager(|tm| tm.current_task_id())
}

/// Task trampoline function.
///
/// This function is the actual entry point for spawned tasks.
/// It takes ownership of the boxed closure, executes it, and then exits.
extern "C" fn task_trampoline<F: FnOnce()>(closure_ptr: usize) -> ! {
    // Reconstruct the box from raw pointer
    let closure = unsafe { Box::from_raw(closure_ptr as *mut F) };
    
    // Execute the closure
    closure();
    
    // Explicitly exit the task after closure completes
    super::exit_current_task();
}

/// Get task trampoline function pointer (helper for spawn_main_task).
pub(super) fn task_trampoline_fn() -> *const () {
    task_trampoline::<Box<dyn FnOnce()>> as *const ()
}
