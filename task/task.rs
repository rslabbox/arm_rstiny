//! Task definition and related types.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use kspin::SpinNoIrq;

use crate::config::kernel::TASK_STACK_SIZE;
use crate::hal::context::TaskContext;

use super::scheduler::fifo_scheduler::FifoTask;

// Forward declaration for TaskRef used in waiters
type WaiterRef = Arc<FifoTask<TaskInner>>;

/// Task identifier type.
pub type TaskId = usize;

/// PID of the tasks
static TASK_PID: AtomicUsize = AtomicUsize::new(1);

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
    /// Tasks waiting for this task to exit (for join support).
    waiters: SpinNoIrq<Vec<WaiterRef>>,
    /// is idle task
    is_idle: bool,
}

// Safety: TaskInner is designed to be shared across threads with proper synchronization.
unsafe impl Send for TaskInner {}
unsafe impl Sync for TaskInner {}

impl TaskInner {
    /// Creates a new task with the given parameters.
    pub fn new(
        id: TaskId,
        name: &'static str,
        parent_id: TaskId,
        is_idle: bool,
        entry: fn(),
    ) -> Self {
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
            is_idle,
            entry: Some(entry),
            waiters: SpinNoIrq::new(Vec::new()),
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

    /// Atomically transition from `expected` state to `new` state.
    ///
    /// Returns `true` if the transition succeeded (i.e., the previous state was
    /// `expected`), or `false` if the state was something else.
    #[inline]
    pub fn try_set_state(&self, expected: TaskState, new: TaskState) -> bool {
        self.state
            .compare_exchange(
                expected as u8,
                new as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
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

    /// Checks if this is an idle task.
    ///
    /// Idle tasks have IDs in the range 0..MAX_CPUS (one per CPU).
    #[inline]
    pub fn is_idle(&self) -> bool {
        self.is_idle
    }

    /// Adds a task to the waiters list (tasks waiting for this task to exit).
    pub fn add_waiter(&self, waiter: WaiterRef) {
        self.waiters.lock().push(waiter);
    }

    /// Takes all waiters from this task (used when task exits).
    pub fn take_waiters(&self) -> Vec<WaiterRef> {
        core::mem::take(&mut *self.waiters.lock())
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

/// Creates a new task and returns its TaskRef.
pub fn create_task(name: &'static str, entry: fn(), is_idle: bool) -> FifoTask<TaskInner> {
    let id = TASK_PID.fetch_add(1, Ordering::SeqCst);

    let parent_id = if !is_idle {
        crate::hal::percpu::current_task().id()
    } else {
        0usize
    };
    let task_inner = TaskInner::new(id, name, parent_id, is_idle, entry);

        info!(
        "Task Created: id={}, name={}, parent_id={}, is_idle={}",
        id, name, parent_id, is_idle
    );
    FifoTask::new(task_inner)
}
