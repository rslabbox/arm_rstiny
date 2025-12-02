pub mod manager;
pub mod task_ops;
pub mod task_ref;
pub mod thread;
pub mod timers;

use alloc::sync::Arc;

use crate::hal::percpu;
// Re-export commonly used types and functions
pub use crate::hal::percpu::current_task;

pub use crate::task::{manager::FifoTask, task_ref::TaskInner};
pub type SchedulableTask = FifoTask<TaskInner>;
pub type TaskRef = Arc<SchedulableTask>;

pub fn start_scheduling() -> ! {
    task_ops::task_start();
}

pub fn init_taskmanager() {
    percpu::set_current_task(&task_ops::get_idle_task());

    info!("Task manager initialized on CPU 0");
}

pub fn init_taskmanager_secondary(cpu_id: usize) {
    // manager::init_secondary();
    percpu::set_current_task(&task_ops::get_idle_task());

    info!("Task manager initialized on CPU {}", cpu_id);
}
