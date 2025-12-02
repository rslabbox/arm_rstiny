use core::{
    any::Any,
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, Ordering},
};

use alloc::{boxed::Box, vec::Vec};
use kspin::SpinNoIrq;

use crate::{
    config::kernel::TASK_STACK_SIZE,
    hal::{context::TaskContext, percpu},
    task::TaskRef,
};

/// Task identifier type.
pub type TaskId = usize;

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
    /// Entry function pointer (type-erased closure that returns a boxed Any).
    entry: Option<Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>>,
    /// Task result (type-erased return value).
    result: SpinNoIrq<Option<Box<dyn Any + Send>>>,
    /// is idle task
    is_idle: bool,
}

// Safety: TaskInner is designed to be shared across threads with proper synchronization.
unsafe impl Send for TaskInner {}
unsafe impl Sync for TaskInner {}

impl TaskInner {
    /// Creates a new task with the given parameters.
    pub fn new<F, T>(
        id: TaskId,
        name: &'static str,
        parent_id: TaskId,
        is_idle: bool,
        entry: F,
    ) -> Self
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
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

        // Wrap the entry function to return a type-erased Box<dyn Any + Send>
        let wrapped_entry: Box<dyn FnOnce() -> Box<dyn Any + Send> + Send> =
            Box::new(move || Box::new(entry()) as Box<dyn Any + Send>);

        Self {
            id,
            name,
            state: AtomicU8::new(TaskState::Ready as u8),
            parent_id,
            children: SpinNoIrq::new(Vec::new()),
            context: UnsafeCell::new(context),
            kstack: Some(kstack),
            is_idle,
            entry: Some(wrapped_entry),
            result: SpinNoIrq::new(None),
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

    /// Takes the entry function out of the task.
    /// Returns None if the entry has already been taken.
    #[inline]
    pub fn take_entry(&self) -> Option<Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>> {
        // Safety: We use interior mutability pattern here.
        // This is safe because entry is only taken once during task execution.
        let ptr = &self.entry as *const _ as *mut Option<Box<dyn FnOnce() -> Box<dyn Any + Send> + Send>>;
        unsafe { (*ptr).take() }
    }

    /// Sets the task result (type-erased return value).
    #[inline]
    pub fn set_result(&self, result: Box<dyn Any + Send>) {
        *self.result.lock() = Some(result);
    }

    /// Takes the task result out.
    /// Returns None if the result has not been set or has already been taken.
    #[inline]
    pub fn take_result(&self) -> Option<Box<dyn Any + Send>> {
        self.result.lock().take()
    }

    /// Checks if this is an idle task.
    ///
    /// Idle tasks have IDs in the range 0..MAX_CPUS (one per CPU).
    #[inline]
    pub fn is_idle(&self) -> bool {
        self.is_idle
    }

    pub fn switch_to(&self, next: &TaskRef) {
        percpu::set_current_task(&next);
        unsafe {
            (*self.context_mut()).switch_to(next.context());
        }
    }
}

/// Task entry trampoline function.
///
/// This function is the actual entry point for all tasks. It retrieves
/// the task's entry function and calls it, then handles task exit.
#[unsafe(no_mangle)]
extern "C" fn task_entry_trampoline() {
    let task = super::current_task();

    // Take and call the actual entry function, storing the result
    if let Some(entry) = task.take_entry() {
        let result = entry();
        task.set_result(result);
    }

    // Task completed, exit
    super::task_ops::task_exit(task);
}
