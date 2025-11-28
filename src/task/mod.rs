pub mod manager;
pub mod task_ops;
pub mod task_ref;
pub mod thread;

use alloc::sync::Arc;

pub use thread::{current_id, sleep, spawn, yield_now};

use crate::hal::percpu;
// Re-export commonly used types and functions
pub use crate::hal::percpu::current_task;

pub use crate::task::{manager::FifoTask, task_ref::TaskInner};
pub type SchedulableTask = FifoTask<TaskInner>;
pub type TaskRef = Arc<SchedulableTask>;

pub fn start_scheduling() -> ! {
    task_ops::task_start();
    // If no tasks, just idle
    task_ops::idle_loop();
}

pub fn init_taskmanager() {
    percpu::set_current_task(&manager::IDLE_TASK);

    info!("Task manager initialized on CPU 0");
}

pub fn init_taskmanager_secondary(cpu_id: usize) {
    // manager::init_secondary();
    percpu::set_current_task(&manager::IDLE_TASK);

    info!("Task manager initialized on CPU {}", cpu_id);
}
