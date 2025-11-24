//! Task control block and state management.

use alloc::string::String;
use alloc::vec::Vec;

use super::context::TaskContext;

/// Task identifier.
pub type TaskId = usize;

/// Task state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Ready to run, waiting in the ready queue.
    Ready,
    /// Currently running on the CPU.
    Running,
    /// Blocked, waiting for an event (e.g., sleep).
    Blocked,
    /// Task has finished execution.
    Dead,
}

/// Task control block.
pub struct Task {
    /// Unique task identifier.
    pub id: TaskId,
    /// Task name for debugging.
    pub name: String,
    /// Saved CPU context.
    pub context: TaskContext,
    /// Task's kernel stack.
    pub stack: Vec<u8>,
    /// Current state.
    pub state: TaskState,
    /// Remaining time slice in ticks.
    pub time_slice: usize,
    /// Wakeup time in nanoseconds (for sleeping tasks).
    pub wakeup_time: Option<u64>,
}

impl Task {
    /// Create a new task.
    ///
    /// # Arguments
    /// * `id` - Unique task ID
    /// * `name` - Task name for debugging
    /// * `entry` - Task entry point address
    /// * `arg` - Argument to pass to the task
    /// * `stack_size` - Size of the task stack in bytes
    pub fn new(id: TaskId, name: String, entry: usize, arg: usize, stack_size: usize) -> Self {
        // Allocate task stack
        let mut stack = Vec::with_capacity(stack_size);
        stack.resize(stack_size, 0);

        // Calculate stack top (stacks grow downward)
        let stack_top = stack.as_ptr() as usize + stack.len();

        // Initialize task context
        let context = TaskContext::new(entry, arg, stack_top);

        Self {
            id,
            name,
            context,
            stack,
            state: TaskState::Ready,
            time_slice: 0, // Will be set by scheduler
            wakeup_time: None,
        }
    }

    /// Check if the task is ready to run.
    pub fn is_ready(&self) -> bool {
        self.state == TaskState::Ready
    }

    /// Check if the task should be woken up.
    pub fn should_wakeup(&self, current_time: u64) -> bool {
        if let Some(wakeup) = self.wakeup_time {
            current_time >= wakeup
        } else {
            false
        }
    }
}

impl core::fmt::Debug for Task {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Task")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("state", &self.state)
            .field("time_slice", &self.time_slice)
            .field("wakeup_time", &self.wakeup_time)
            .field("pc", &format_args!("{:#x}", self.context.pc))
            .field("sp", &format_args!("{:#x}", self.context.sp))
            .finish()
    }
}
