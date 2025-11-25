//! Task manager implementation.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use kspin::SpinNoIrq;

use crate::drivers::timer::current_nanoseconds;

use super::scheduler::fifo_scheduler::FifoTask;
use super::scheduler::BaseScheduler;
use super::task::{TaskId, TaskInner, TaskRef, TaskState, ROOT_ID};
use super::wait_queue::WaitQueue;
use super::Scheduler;

/// Global task manager instance.
static TASK_MANAGER: SpinNoIrq<Option<TaskManager>> = SpinNoIrq::new(None);

/// Task manager that handles all task-related operations.
pub struct TaskManager {
    /// Currently running task.
    current: TaskRef,
    /// FIFO scheduler for ready tasks.
    scheduler: Scheduler,
    /// Wait queue for sleeping tasks.
    wait_queue: WaitQueue,
    /// Map of all tasks by ID.
    tasks: BTreeMap<TaskId, TaskRef>,
    /// Next available task ID.
    next_id: TaskId,
    /// Whether the task manager has been initialized.
    initialized: bool,
    /// Whether scheduling has started (after start_scheduling is called).
    scheduling_started: bool,
}

impl TaskManager {
    /// Creates a new task manager with ROOT task.
    fn new() -> Self {
        // Create ROOT task
        let root_inner = TaskInner::new_root();
        let root_task = Arc::new(FifoTask::new(root_inner));
        
        let mut tasks = BTreeMap::new();
        tasks.insert(ROOT_ID, root_task.clone());
        
        let mut scheduler = Scheduler::new();
        scheduler.init();
        
        Self {
            current: root_task,
            scheduler,
            wait_queue: WaitQueue::new(),
            tasks,
            next_id: 1, // Start from 1, ROOT is 0
            initialized: true,
            scheduling_started: false,
        }
    }

    /// Returns whether the task manager is initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Returns a reference to the current task.
    pub fn current(&self) -> &TaskRef {
        &self.current
    }

    /// Spawns a new task with the given entry function.
    /// 
    /// The new task becomes a child of the current task.
    pub fn spawn(&mut self, name: &'static str, entry: fn()) -> TaskId {
        let id = self.next_id;
        self.next_id += 1;
        
        let parent_id = self.current.id();
        let task_inner = TaskInner::new(id, name, parent_id, entry);
        let task = Arc::new(FifoTask::new(task_inner));
        
        // Add to tasks map
        self.tasks.insert(id, task.clone());
        
        // Add as child of current task
        self.current.add_child(id);
        
        // Add to scheduler ready queue
        self.scheduler.add_task(task);
        
        info!("Task {} ({}) spawned, parent: {}", id, name, parent_id);
        
        id
    }

    /// Puts the current task to sleep for the specified duration.
    pub fn sleep_current(&mut self, duration_ns: u64) {
        let wake_time = current_nanoseconds() + duration_ns;
        
        // Set current task state to sleeping
        self.current.set_state(TaskState::Sleeping);
        
        // Add to wait queue
        self.wait_queue.add(self.current.clone(), wake_time);
        
        // Schedule next task
        self.schedule_next();
    }

    /// Current task voluntarily yields CPU.
    pub fn yield_current(&mut self) {
        // Put current back to ready queue
        self.current.set_state(TaskState::Ready);
        self.scheduler.put_prev_task(self.current.clone(), false);
        
        // Schedule next task
        self.schedule_next();
    }

    /// Exits the current task.
    pub fn exit_current(&mut self) {
        let current_id = self.current.id();
        
        if current_id == ROOT_ID {
            panic!("Cannot exit ROOT task!");
        }
        
        info!("Task {} ({}) exiting", current_id, self.current.name());
        
        // Set state to exited
        self.current.set_state(TaskState::Exited);
        
        // Transfer children to ROOT
        let children = self.current.take_children();
        if !children.is_empty() {
            if let Some(root) = self.tasks.get(&ROOT_ID) {
                for child_id in children {
                    root.add_child(child_id);
                    // Update child's parent reference if needed
                    // (we don't store mutable parent_id, so this is informational only)
                }
            }
        }
        
        // Remove from parent's children list
        let parent_id = self.current.parent_id();
        if let Some(parent) = self.tasks.get(&parent_id) {
            parent.remove_child(current_id);
        }
        
        // Remove from tasks map
        self.tasks.remove(&current_id);
        
        // Schedule next task (current will be dropped when no more references)
        self.schedule_next();
    }

    /// Wakes up expired tasks from the wait queue.
    fn wake_expired_tasks(&mut self) {
        let current_ns = current_nanoseconds();
        let woken = self.wait_queue.wake_expired(current_ns);
        
        for task in woken {
            task.set_state(TaskState::Ready);
            self.scheduler.add_task(task);
        }
    }

    /// Schedules the next task to run.
    fn schedule_next(&mut self) {
        // First, wake up any expired sleeping tasks
        self.wake_expired_tasks();
        
        // Try to pick next task from scheduler
        let next = self.scheduler.pick_next_task();
        
        let next_task = match next {
            Some(task) => task,
            None => {
                // No ready tasks, switch to ROOT (idle)
                self.tasks.get(&ROOT_ID).expect("ROOT task must exist").clone()
            }
        };
        
        // If switching to the same task, do nothing
        if Arc::ptr_eq(&self.current, &next_task) {
            self.current.set_state(TaskState::Running);
            return;
        }
        
        // Perform context switch
        let prev = core::mem::replace(&mut self.current, next_task);
        self.current.set_state(TaskState::Running);
        
        // Context switch
        unsafe {
            prev.context_mut().switch_to(self.current.context());
        }
    }

    /// Called from timer interrupt to check for scheduling.
    pub fn timer_tick(&mut self) {
        // Don't schedule if scheduling hasn't started yet
        if !self.scheduling_started {
            return;
        }

        // Wake expired tasks
        self.wake_expired_tasks();
        
        // Check if current task should be preempted
        let should_preempt = self.scheduler.task_tick(&self.current);
        
        if should_preempt || self.current.state() != TaskState::Running {
            // Put current task back if it's still running
            if self.current.state() == TaskState::Running {
                self.current.set_state(TaskState::Ready);
                self.scheduler.put_prev_task(self.current.clone(), true);
            }
            
            self.schedule_next();
        }
    }

    /// Starts scheduling - switches to the first ready task.
    /// This function never returns.
    pub fn start(&mut self) -> ! {
        info!("Starting scheduler...");
        
        // Mark scheduling as started
        self.scheduling_started = true;
        
        // Get first task from scheduler
        if let Some(first_task) = self.scheduler.pick_next_task() {
            let prev = core::mem::replace(&mut self.current, first_task);
            self.current.set_state(TaskState::Running);
            
            info!("Switching to task {} ({})", self.current.id(), self.current.name());
            
            // Context switch from ROOT to first task
            unsafe {
                prev.context_mut().switch_to(self.current.context());
            }
        }
        
        // If no tasks, just idle
        idle_loop();
    }
}

/// Idle loop for ROOT task when no other tasks are ready.
fn idle_loop() -> ! {
    loop {
        // Wait for interrupt
        aarch64_cpu::asm::wfi();
    }
}

// ============================================================================
// Public API functions
// ============================================================================

/// Initializes the task manager.
pub fn init() {
    info!("Initializing task manager...");
    
    let manager = TaskManager::new();
    *TASK_MANAGER.lock() = Some(manager);
    
    info!("Task manager initialized");
}

/// Returns whether the task manager is initialized.
pub fn is_initialized() -> bool {
    TASK_MANAGER.lock().as_ref().map_or(false, |m| m.is_initialized())
}

/// Returns the current task reference.
pub fn current_task() -> TaskRef {
    TASK_MANAGER.lock()
        .as_ref()
        .expect("Task manager not initialized")
        .current()
        .clone()
}

/// Spawns a new task.
pub fn spawn(name: &'static str, entry: fn()) -> TaskId {
    TASK_MANAGER.lock()
        .as_mut()
        .expect("Task manager not initialized")
        .spawn(name, entry)
}

/// Puts the current task to sleep.
pub fn sleep(duration_ns: u64) {
    TASK_MANAGER.lock()
        .as_mut()
        .expect("Task manager not initialized")
        .sleep_current(duration_ns);
}

/// Current task yields CPU.
pub fn yield_now() {
    TASK_MANAGER.lock()
        .as_mut()
        .expect("Task manager not initialized")
        .yield_current();
}

/// Exits the current task.
pub fn exit_current() {
    TASK_MANAGER.lock()
        .as_mut()
        .expect("Task manager not initialized")
        .exit_current();
}

/// Called from timer interrupt.
pub fn on_timer_tick() {
    if let Some(manager) = TASK_MANAGER.lock().as_mut() {
        manager.timer_tick();
    }
}

/// Starts the scheduler. Never returns.
pub fn start_scheduling() -> ! {
    TASK_MANAGER.lock()
        .as_mut()
        .expect("Task manager not initialized")
        .start()
}
