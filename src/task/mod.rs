//! Task management module.
//!
//! This module provides multi-tasking support including:
//! - Task creation and management
//! - Cooperative and preemptive scheduling
//! - Sleep and yield operations
//!
//! # Task Hierarchy
//!
//! - ROOT (ID=0): The idle task, created at initialization. Uses the bootstrap stack.
//! - user_main (ID=1): The main user task, child of ROOT.
//! - Other tasks: Created via `spawn()`, become children of the creating task.

pub mod manager;
pub mod scheduler;
pub mod task;
pub mod thread;

use scheduler::fifo_scheduler::FifoScheduler;
use task::TaskInner;

/// Type alias for the scheduler implementation.
pub type Scheduler = FifoScheduler<TaskInner>;

// Re-export commonly used types and functions
pub use manager::{
    exit_current as exit_current_task, init as init_taskmanager,
    is_initialized, on_timer_tick as schedule, sleep, spawn as spawn_task,
    start_scheduling, yield_now,
};
pub use crate::hal::percpu::{current_task};
pub use task::{TaskId, TaskRef, TaskState, ROOT_ID};

/// Spawns a new task with the given entry function.
///
/// This is a convenience wrapper that creates a named task.
pub fn spawn(entry: fn()) {
    spawn_task("task", entry);
}
