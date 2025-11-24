//! Task/Process management module.
//!
//! This module provides:
//! - Task control blocks (TCB)
//! - Priority-based scheduler with bitmap optimization
//! - Context switching
//! - Task creation and management

mod context;
pub mod scheduler;
mod task;

pub use scheduler::{current_task, exit, yield_now};
pub use task::{TaskControlBlock, TaskId, TaskState};

use alloc::sync::Arc;
use spin::Mutex;

use crate::config::kernel::{DEFAULT_TASK_STACK_SIZE, DEFAULT_TIME_SLICE_MS, MAX_TASK_PRIORITY};

/// Task reference type
pub type TaskRef = Arc<Mutex<TaskControlBlock>>;

/// Create a new task
///
/// # Arguments
/// * `entry` - Task entry function
/// * `priority` - Task priority (0 is highest, 31 is lowest)
/// * `stack_size` - Optional stack size (defaults to DEFAULT_TASK_STACK_SIZE)
///
/// # Returns
/// Task ID on success, error message on failure
pub fn spawn(entry: fn() -> !, priority: u8, stack_size: Option<usize>) -> Result<TaskId, &'static str> {
    if priority > MAX_TASK_PRIORITY {
        return Err("Priority out of range");
    }
    
    let stack_size = stack_size.unwrap_or(DEFAULT_TASK_STACK_SIZE);
    let tid = task::alloc_tid();
    
    let tcb = TaskControlBlock::new(
        tid,
        entry as usize,
        priority,
        stack_size,
        DEFAULT_TIME_SLICE_MS,
    );
    
    let task_ref = Arc::new(Mutex::new(tcb));
    scheduler::add_task(task_ref);
    
    info!("Task {} created with priority {}", tid, priority);
    Ok(tid)
}

/// Initialize the task system
pub fn init() {
    info!("Task system initialized");
}
