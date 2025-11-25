//! Task scheduling module.
//!
//! This module provides cooperative multitasking with time-slice round-robin scheduling.

mod manager;
mod task;
pub mod tests;
pub mod thread;

use alloc::boxed::Box;

use kspin::SpinNoIrq;
use lazyinit::LazyInit;

use crate::hal::TrapFrame;
use manager::TaskManager;

/// Global task manager instance.
static TASK_MANAGER: LazyInit<SpinNoIrq<TaskManager>> = LazyInit::new();

/// Flag indicating if task manager is initialized.
static mut INITIALIZED: bool = false;

/// Initialize the task manager.
///
/// Creates the ROOT task and prepares the task manager for use.
pub fn init_taskmanager() {
    info!("Initializing task manager...");

    let task_manager = TaskManager::new();
    TASK_MANAGER.init_once(SpinNoIrq::new(task_manager));

    unsafe {
        INITIALIZED = true;
    }

    info!("Task manager initialized");
}

/// Check if task manager is initialized.
pub fn is_initialized() -> bool {
    unsafe { INITIALIZED }
}

/// Spawn the main user task.
///
/// This creates a user task as a child of the ROOT task.
///
/// # Arguments
/// * `main_fn` - The entry point function for the main task
pub fn spawn_main_task(main_fn: fn()) -> task::TaskId {
    info!("Creating main user task...");

    with_task_manager(|tm| {
        // Create a wrapper closure
        let closure_ptr = Box::into_raw(Box::new(move || {
            main_fn();
            info!("User main completed");
            // Don't exit, just sleep forever
            loop {
                thread::sleep(core::time::Duration::from_secs(3600));
            }
        })) as usize;

        // Spawn with ROOT as parent
        let task_id = tm.spawn_with_parent(
            alloc::string::String::from("user_main"),
            thread::task_trampoline_fn() as usize,
            closure_ptr,
            Some(tm.root_task_id),
        );

        info!("Main user task created with ID: {}", task_id.as_usize());
        task_id
    })
}

/// Start the task scheduler.
///
/// This transfers control to the ROOT task and begins scheduling.
/// This function will not return.
pub fn start_scheduling() -> ! {
    info!("Starting task scheduler, transferring control to ROOT...");
    
    // Create initial TrapFrame
    let mut tf = TrapFrame::default();
    
    with_task_manager(|tm| {
        // Set ROOT as current task
        tm.current_task = Some(tm.root_task_id);
        
        // Trigger first schedule (should pick user_main)
        tm.schedule(&mut tf);
    });
    
    info!("First task scheduled, jumping...");
    
    // Load context and jump to first task
    unsafe {
        core::arch::asm!(
            "mov sp, {sp}",
            "ldp x0, x1, [sp]",
            "ldp x2, x3, [sp, 2 * 8]",
            "ldp x4, x5, [sp, 4 * 8]",
            "ldp x6, x7, [sp, 6 * 8]",
            "ldp x8, x9, [sp, 8 * 8]",
            "ldp x10, x11, [sp, 10 * 8]",
            "ldp x12, x13, [sp, 12 * 8]",
            "ldp x14, x15, [sp, 14 * 8]",
            "ldp x16, x17, [sp, 16 * 8]",
            "ldp x18, x19, [sp, 18 * 8]",
            "ldp x20, x21, [sp, 20 * 8]",
            "ldp x22, x23, [sp, 22 * 8]",
            "ldp x24, x25, [sp, 24 * 8]",
            "ldp x26, x27, [sp, 26 * 8]",
            "ldp x28, x29, [sp, 28 * 8]",
            "ldp x30, x9, [sp, 30 * 8]",
            "msr sp_el0, x9",
            "ldp x10, x11, [sp, 32 * 8]",
            "msr elr_el1, x10",
            "msr spsr_el1, x11",
            "ldp x0, x1, [sp]",
            "eret",
            sp = in(reg) &tf,
            options(noreturn)
        );
    }
}

/// Schedule tasks (called from interrupt handler).
///
/// # Arguments
/// * `tf` - Trap frame from the interrupt
pub fn schedule(tf: &mut TrapFrame) {
    with_task_manager(|tm| {
        tm.schedule(tf);
    });
}

/// Wake up a sleeping task.
///
/// This is called from timer callbacks to wake up tasks.
pub fn wake_task(task_id: task::TaskId) {
    with_task_manager(|tm| {
        tm.wake_task(task_id);
    });
}

/// Exit the current task.
///
/// This will reschedule and never return.
pub fn exit_current_task() -> ! {
    // This will be handled by triggering a context switch
    // For now, just loop with wfi
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

/// Helper function to access task manager with lock.
fn with_task_manager<F, R>(f: F) -> R
where
    F: FnOnce(&mut TaskManager) -> R,
{
    let mut tm = TASK_MANAGER.lock();
    f(&mut tm)
}