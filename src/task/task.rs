//! Task definition and related types.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

use kspin::SpinNoIrq;

use crate::config::kernel::TASK_STACK_SIZE;
use crate::hal::context::TaskContext;

use super::scheduler::fifo_scheduler::FifoTask;

/// Task identifier type.
pub type TaskId = usize;

/// Root task ID (idle task).
pub const ROOT_ID: TaskId = 0;

/// Task state enumeration.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Task is in the ready queue, waiting to be scheduled.
    Ready = 0,
    /// Task is currently running on CPU.
    Running = 1,
    /// Task is sleeping, waiting for a timer.
    Sleeping = 2,
    /// Task has exited and is waiting for cleanup.
    Exited = 3,
}

impl From<u8> for TaskState {
    fn from(val: u8) -> Self {
        match val {
            0 => TaskState::Ready,
            1 => TaskState::Running,
            2 => TaskState::Sleeping,
            3 => TaskState::Exited,
            _ => TaskState::Ready,
        }
    }
}

/// Inner task structure containing all task metadata.
#[allow(unused)]
pub struct TaskInner {
    /// Unique task identifier.
    id: TaskId,
    /// Task name for debugging.
    name: &'static str,
    /// Current task state (atomic for safe concurrent access).
    state: AtomicU8,
    /// Parent task ID.
    parent_id: TaskId,
    /// List of child task IDs.
    children: SpinNoIrq<Vec<TaskId>>,
    /// Task context (registers, stack pointer, etc.).
    context: UnsafeCell<TaskContext>,
    /// Kernel stack for this task. None for ROOT which uses bootstrap stack.
    kstack: Option<Box<[u8]>>,
    /// Entry function pointer.
    entry: Option<fn()>,
}

// Safety: TaskInner is designed to be shared across threads with proper synchronization.
unsafe impl Send for TaskInner {}
unsafe impl Sync for TaskInner {}

impl TaskInner {
    /// Creates the ROOT (idle) task.
    ///
    /// The ROOT task reuses the bootstrap stack and has no parent.
    pub fn new_root() -> Self {
        Self {
            id: ROOT_ID,
            name: "ROOT",
            state: AtomicU8::new(TaskState::Running as u8),
            parent_id: ROOT_ID, // ROOT is its own parent
            children: SpinNoIrq::new(Vec::new()),
            context: UnsafeCell::new(TaskContext::new()),
            kstack: None, // Reuse bootstrap stack
            entry: None,
        }
    }

    /// Creates a new task with the given parameters.
    pub fn new(id: TaskId, name: &'static str, parent_id: TaskId, entry: fn()) -> Self {
        // Allocate kernel stack
        let kstack = alloc::vec![0u8; TASK_STACK_SIZE].into_boxed_slice();
        let kstack_top = kstack.as_ptr() as usize + TASK_STACK_SIZE;

        let mut context = TaskContext::new();
        // Initialize context with entry point and stack
        context.init(
            task_entry_trampoline as *const () as usize,
            memory_addr::VirtAddr::from(kstack_top),
            memory_addr::VirtAddr::from(0usize), // No TLS for now
        );

        Self {
            id,
            name,
            state: AtomicU8::new(TaskState::Ready as u8),
            parent_id,
            children: SpinNoIrq::new(Vec::new()),
            context: UnsafeCell::new(context),
            kstack: Some(kstack),
            entry: Some(entry),
        }
    }

    /// Returns the task ID.
    #[inline]
    pub fn id(&self) -> TaskId {
        self.id
    }

    /// Returns the task name.
    #[inline]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the current task state.
    #[inline]
    pub fn state(&self) -> TaskState {
        TaskState::from(self.state.load(Ordering::Acquire))
    }

    /// Sets the task state.
    #[inline]
    pub fn set_state(&self, state: TaskState) {
        self.state.store(state as u8, Ordering::Release);
    }

    /// Returns a mutable reference to the task context.
    ///
    /// # Safety
    /// Caller must ensure exclusive access to the context.
    #[inline]
    pub unsafe fn context_mut(&self) -> &mut TaskContext {
        unsafe { &mut *self.context.get() }
    }

    /// Returns a reference to the task context.
    #[inline]
    pub fn context(&self) -> &TaskContext {
        unsafe { &*self.context.get() }
    }

    /// Returns the entry function if any.
    #[inline]
    pub fn entry(&self) -> Option<fn()> {
        self.entry
    }

    /// Checks if this is the idle task (alias for is_root).
    #[inline]
    pub fn is_idle(&self) -> bool {
        self.id == ROOT_ID
    }
}

/// Type alias for schedulable task (wrapped in FifoTask for intrusive list).
pub type SchedulableTask = FifoTask<TaskInner>;

/// Type alias for task reference (Arc-wrapped schedulable task).
pub type TaskRef = Arc<SchedulableTask>;

/// Task entry trampoline function.
///
/// This function is the actual entry point for all tasks. It retrieves
/// the task's entry function and calls it, then handles task exit.
#[unsafe(no_mangle)]
extern "C" fn task_entry_trampoline() {
    let task = super::current_task();

    // Call the actual entry function
    if let Some(entry) = task.entry() {
        entry();
    }

    // Task completed, exit
    super::exit_current_task();
}
