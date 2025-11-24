//! Task management and scheduling.

mod context;
mod scheduler;
mod task;
pub mod thread;

use spin::Mutex;

use scheduler::Scheduler;

use crate::hal::TrapFrame;

/// Global scheduler instance.
static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Global sleep request (simplified implementation).
static SLEEP_REQUEST: Mutex<Option<u64>> = Mutex::new(None);

/// Initialize the scheduler.
pub fn init_scheduler() {
    let mut scheduler = SCHEDULER.lock();
    *scheduler = Some(Scheduler::new());
    info!("Task scheduler initialized");
}

/// Get a lock on the scheduler.
pub(crate) fn scheduler_lock() -> spin::MutexGuard<'static, Option<Scheduler>> {
    SCHEDULER.lock()
}

/// Handle timer tick - called from timer interrupt.
pub fn tick() {
    if let Some(scheduler) = SCHEDULER.lock().as_mut() {
        scheduler.tick();
    }
}

/// Perform scheduling - called from interrupt handler with trap frame.
pub fn schedule(tf: &mut TrapFrame) {
    // Check for pending sleep request
    let sleep_ns = SLEEP_REQUEST.lock().take();
    
    if let Some(scheduler) = SCHEDULER.lock().as_mut() {
        if let Some(duration) = sleep_ns {
            scheduler.sleep_current(duration, tf);
        } else {
            scheduler.schedule(tf);
        }
    }
}

/// Yield the current task - called from interrupt handler.
pub fn yield_current(tf: &mut TrapFrame) {
    if let Some(scheduler) = SCHEDULER.lock().as_mut() {
        scheduler.yield_current(tf);
    }
}

/// Set a sleep request for the current task.
pub(crate) fn set_sleep_request(duration_ns: u64) {
    *SLEEP_REQUEST.lock() = Some(duration_ns);
}

/// Check if scheduler is initialized.
pub fn is_initialized() -> bool {
    SCHEDULER.lock().is_some()
}
