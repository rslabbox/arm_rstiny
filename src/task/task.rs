//! Task control block and task management.

use alloc::{string::String, vec::Vec};

use crate::hal::TrapFrame;

/// Unique task identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(pub usize);

impl TaskId {
    pub const fn new(id: usize) -> Self {
        Self(id)
    }

    pub const fn as_usize(&self) -> usize {
        self.0
    }
}

/// Task execution state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task is ready to run.
    Ready,
    /// Task is currently running.
    Running,
    /// Task is sleeping.
    Sleeping,
    /// Task has exited.
    Exited,
}

/// Task control block.
#[allow(dead_code)]
pub struct Task {
    /// Task identifier.
    pub id: TaskId,
    /// Task name.
    pub name: String,
    /// Task state.
    pub state: TaskState,
    /// Saved task context.
    pub context: TrapFrame,
    /// Task stack.
    pub stack: Vec<u8>,
    /// Stack top pointer (used for context switching).
    pub stack_top: usize,
    /// Parent task ID.
    pub parent_id: Option<TaskId>,
    /// Child task IDs.
    pub children: Vec<TaskId>,
    /// Remaining time slice ticks.
    pub ticks_remaining: usize,
}

impl Task {
    /// Create the idle task.
    pub fn new_idle() -> Self {
        let mut stack = Vec::with_capacity(crate::config::kernel::TASK_STACK_SIZE);
        stack.resize(crate::config::kernel::TASK_STACK_SIZE, 0);
        let stack_top = stack.as_ptr() as usize + stack.len();

        Self {
            id: TaskId::new(0),
            name: String::from("idle"),
            state: TaskState::Ready,
            context: TrapFrame::default(),
            stack,
            stack_top,
            parent_id: None,
            children: Vec::new(),
            ticks_remaining: crate::config::kernel::DEFAULT_TIME_SLICE,
        }
    }

    /// Create a new task.
    pub fn new(id: TaskId, name: String, parent_id: Option<TaskId>) -> Self {
        let mut stack = Vec::with_capacity(crate::config::kernel::TASK_STACK_SIZE);
        stack.resize(crate::config::kernel::TASK_STACK_SIZE, 0);
        let stack_top = stack.as_ptr() as usize + stack.len();

        Self {
            id,
            name,
            state: TaskState::Ready,
            context: TrapFrame::default(),
            stack,
            stack_top,
            parent_id,
            children: Vec::new(),
            ticks_remaining: crate::config::kernel::DEFAULT_TIME_SLICE,
        }
    }

    /// Initialize task context.
    ///
    /// Sets up the initial execution context for the task:
    /// - PC (elr): entry point address
    /// - SP: stack top pointer
    /// - Argument register (x0): argument value
    /// - Link register (x30): exit handler address
    pub fn init_context(&mut self, entry: usize, arg: usize, exit_handler: usize) {
        // Set program counter to entry point
        self.context.elr = entry as u64;
        
        // Set stack pointer (aligned to 16 bytes)
        self.context.usp = (self.stack_top & !0xf) as u64;
        
        // Set argument in x0
        self.context.r[0] = arg as u64;
        
        // Set link register to exit handler
        self.context.r[30] = exit_handler as u64;
        
        // Set SPSR to EL1h with interrupts enabled
        // SPSR_EL1: M[4:0] = 0b00101 (EL1h), D=0, A=0, I=0, F=0
        self.context.spsr = 0b00101;
    }
}
