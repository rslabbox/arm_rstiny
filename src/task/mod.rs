//! Task management module.
//!
//! This module provides multi-tasking support including:
//! - Task creation and management
//! - Cooperative and preemptive scheduling
//! - Sleep and yield operations
//! - Multi-core support with shared scheduler
//!
//! # Task Hierarchy
//!
//! - idle_N (ID=N): The idle task for CPU N, created at initialization.
//! - user_main: The main user task, child of idle_0.
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
pub use crate::hal::percpu::current_task;
#[allow(unused)]
pub use manager::{
    exit_current as exit_current_task, init as init_taskmanager,
    init_secondary as init_taskmanager_secondary, is_initialized, on_timer_tick as schedule, sleep,
    spawn as spawn_task, start_scheduling, yield_now,
};

/// Spawns a new task with the given entry function.
///
/// This is a convenience wrapper that creates a named task.
pub fn spawn(entry: fn()) {
    spawn_task("task", entry);
}
