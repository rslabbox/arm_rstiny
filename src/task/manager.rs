//! Task manager implementation.
//!
//! This module provides a clean task management architecture with:
//! - Unified block/unblock mechanism
//! - Single scheduling entry point (resched)
//! - Timer-based sleep using block_current + unblock_task
//! - Multi-core support with shared scheduler

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use kspin::SpinRaw;

use crate::config::kernel::TINYENV_SMP;
use crate::drivers::timer::current_nanoseconds;
use crate::hal::percpu;
use crate::task::task;

use super::Scheduler;
use super::scheduler::BaseScheduler;
use super::scheduler::fifo_scheduler::FifoTask;
use super::task::{TaskInner, TaskRef, TaskState};

/// Global task manager instance.
static TASK_MANAGER: SpinRaw<Option<TaskManager>> = SpinRaw::new(None);

/// Global active task count (excluding idle tasks).
/// Uses atomic for safe multi-core access.
static ACTIVE_TASK_COUNT: AtomicUsize = AtomicUsize::new(0);


lazy_static::lazy_static! {
    /// Public lock for accessing the task manager.
    pub static ref IDLE_TASK: Arc<FifoTask<TaskInner>> = {
        // Placeholder idle task for initialization
        let idle_task = task::create_task("idle",|| idle_loop(), true);
        Arc::new(idle_task)
    };
}

/// Task manager that handles all task scheduling operations.
pub struct TaskManager {
    /// Scheduler for ready tasks.
    scheduler: Scheduler,
}

impl TaskManager {
    /// Creates a new task manager.
    fn new() -> Self {
        let mut scheduler = Scheduler::new();
        scheduler.init();

        info!("Scheduler used: {}", Scheduler::scheduler_name());

        Self {
            scheduler,
        }
    }

    /// Handles scheduler timer tick.
    pub fn scheduler_timer_tick(&mut self) -> bool {
        self.scheduler.task_tick(&percpu::current_task())
    }

    /// Adds a ready task to the scheduler.
    pub fn spawn(&mut self, task: TaskRef) {
        assert!(task.state() == TaskState::Ready);
        self.scheduler.add_task(task);
    }

    /// Switches context from current task to next task.
    fn switch_to(&self, curr_task: &TaskRef, next_task: TaskRef) {
        trace!(
            "context switch: {} ({}) -> {} ({})",
            curr_task.id(),
            curr_task.name(),
            next_task.id(),
            next_task.name()
        );

        next_task.set_state(TaskState::Running);

        // If switching to the same task, do nothing
        if Arc::ptr_eq(curr_task, &next_task) {
            return;
        }

        let next_ctx = next_task.context();

        // Update percpu with next task before context switch
        percpu::set_current_task(&next_task);

        // Context switch
        unsafe {
            (*curr_task.context_mut()).switch_to(next_ctx);
        }
    }

    /// Reschedules - picks next task and switches to it.
    ///
    /// Precondition: current task state is NOT Running.
    fn resched(&mut self, curr_task: &TaskRef) {
        assert!(curr_task.state() != TaskState::Running);

        let next_task = if let Some(task) = self.scheduler.pick_next_task() {
            task
        } else {
            // No ready tasks, switch to idle task
            IDLE_TASK.clone()
        };

        self.switch_to(curr_task, next_task);
    }

    /// Current task voluntarily yields CPU.
    pub fn yield_current(&mut self, curr_task: &TaskRef) {
        assert!(curr_task.state() == TaskState::Running);

        curr_task.set_state(TaskState::Ready);

        // Don't put idle task back to queue
        if !curr_task.is_idle() {
            self.scheduler.put_prev_task(curr_task.clone(), false);
        }

        self.resched(curr_task);
    }

    /// Unblocks a sleeping task by adding it back to the scheduler.
    ///
    /// Returns true if the task was successfully unblocked.
    /// Uses atomic CAS to ensure only one CPU can unblock the task.
    pub fn unblock_task(&mut self, task: TaskRef) -> bool {
        // Atomically transition Sleeping -> Ready; only one CPU can succeed
        if task.try_set_state(TaskState::Sleeping, TaskState::Ready) {
            trace!("Unblocking task {} ({})", task.id(), task.name());
            self.scheduler.add_task(task);
            true
        } else {
            false
        }
    }

    /// Blocks the current task (sets state to Sleeping and reschedules).
    ///
    /// The task will remain blocked until unblock_task is called.
    pub fn block_current(&mut self, curr_task: &TaskRef) {
        assert!(curr_task.state() == TaskState::Running);
        assert!(!curr_task.is_idle());

        curr_task.set_state(TaskState::Sleeping);
        self.resched(curr_task);
    }

    /// Puts the current task to sleep until the specified deadline.
    pub fn sleep_current(&mut self, curr_task: &TaskRef, deadline_ns: u64) {
        assert!(curr_task.state() == TaskState::Running);
        assert!(!curr_task.is_idle());

        // If deadline already passed, don't sleep
        if current_nanoseconds() >= deadline_ns {
            return;
        }

        // Clone task reference for the timer callback
        let task_clone = curr_task.clone();

        // Calculate duration from now to deadline
        let now = current_nanoseconds();
        let duration = Duration::from_nanos(deadline_ns - now);

        // Set a timer to unblock the task
        crate::drivers::timer::set_timer(duration, move |_| {
            TASK_MANAGER.lock().as_mut().map(|manager| {
                manager.unblock_task(task_clone);
            });
        });

        // Block the current task
        self.block_current(curr_task);
    }

    /// Exits the current task.
    pub fn exit_current(&mut self, curr_task: &TaskRef) -> ! {
        assert!(!curr_task.is_idle());
        assert!(curr_task.state() == TaskState::Running);

        info!("Task {} ({}) exiting", curr_task.id(), curr_task.name());

        // Wake up all tasks waiting for this task to exit
        let waiters = curr_task.take_waiters();
        for waiter in waiters {
            trace!(
                "Waking up waiter {} ({}) for task {}",
                waiter.id(),
                waiter.name(),
                curr_task.id()
            );
            self.unblock_task(waiter);
        }

        // Decrement active task count atomically
        let remaining = ACTIVE_TASK_COUNT.fetch_sub(1, Ordering::SeqCst) - 1;

        if remaining == 0 {
            info!("All tasks exited, shutting down...");
            crate::drivers::power::system_off();
        }

        curr_task.set_state(TaskState::Exited);

        self.resched(curr_task);

        unreachable!("task exited!");
    }

    /// Called from timer interrupt to check for preemption.
    pub fn timer_tick(&mut self) {
        let curr_task = percpu::current_task();

        if self.scheduler_timer_tick() {
            // Put current task back if it's still running
            if curr_task.state() == TaskState::Running {
                curr_task.set_state(TaskState::Ready);
                if !curr_task.is_idle() {
                    self.scheduler.put_prev_task(curr_task.clone(), true);
                }
                self.resched(&curr_task);
            }
        }
    }

    /// Starts scheduling - switches to the first ready task.
    /// This function never returns.
    pub fn start(&mut self) -> ! {
        info!("Starting scheduler...");

        // Get first task from scheduler
        if let Some(first_task) = self.scheduler.pick_next_task() {
            let idle = IDLE_TASK.clone();

            info!(
                "Switching to task {} ({})",
                first_task.id(),
                first_task.name()
            );

            self.switch_to(&idle, first_task);
        }

        // If no tasks, just idle
        idle_loop();
    }

    /// Creates a new task and returns its TaskRef.
    pub fn create_task(&mut self, name: &'static str, entry: fn()) -> TaskRef {
        let task = Arc::new(task::create_task(name, entry, false));
        let task_ref = task.clone();

        ACTIVE_TASK_COUNT.fetch_add(1, Ordering::SeqCst);

        // Add to scheduler ready queue
        self.spawn(task);

        task_ref
    }

    /// Joins on a target task - blocks current task until target exits.
    ///
    /// If the target task has already exited, returns immediately.
    pub fn join_task(&mut self, curr_task: &TaskRef, target: &TaskRef) {
        // If target already exited, return immediately
        if target.state() == TaskState::Exited {
            return;
        }

        // Add current task to target's waiters list
        target.add_waiter(curr_task.clone());

        // Block current task
        self.block_current(curr_task);
    }
}

/// Idle loop for ROOT task when no other tasks are ready.
fn idle_loop() -> ! {
    loop {
        // Try to pick a task from the ready queue
        if let Some(manager) = TASK_MANAGER.lock().as_mut() {
            if let Some(next_task) = manager.scheduler.pick_next_task() {
                // Found a task, switch to it
                let idle = IDLE_TASK.clone();
                idle.set_state(TaskState::Ready);
                next_task.set_state(TaskState::Running);
                percpu::set_current_task(&next_task);
                unsafe {
                    (*idle.context_mut()).switch_to(next_task.context());
                }
            }
        }

        info!("CPU {} idle, waiting for interrupt", percpu::cpu_id());
        // No task available, wait for interrupt
        aarch64_cpu::asm::wfi();
    }
}

// ============================================================================
// Public API functions
// ============================================================================

/// Initializes the task manager (called by CPU 0).
pub fn init() {
    info!("Initializing task manager...");

    // Set idle_0 as current task via percpu
    percpu::set_current_task(&IDLE_TASK);

    // Create task manager
    let manager = TaskManager::new();
    *TASK_MANAGER.lock() = Some(manager);

    info!("Task manager initialized on CPU 0");
}

/// Initializes task scheduling for a secondary CPU.
///
/// This function must be called by each secondary CPU before entering the scheduler.
/// It creates the idle task for this CPU and sets up percpu data.
pub fn init_secondary(cpu_id: usize) {
    assert!(cpu_id > 0 && cpu_id < TINYENV_SMP, "Invalid secondary CPU ID");

    // Set as current task and idle task for this CPU
    percpu::set_current_task(&IDLE_TASK);

    info!("Task scheduler initialized on CPU {}", cpu_id);
}

/// Returns whether the task manager is initialized.
pub fn is_initialized() -> bool {
    TASK_MANAGER.lock().is_some()
}

/// Spawns a new task and returns its TaskRef.
pub fn spawn(name: &'static str, entry: fn()) -> TaskRef {
    TASK_MANAGER
        .lock()
        .as_mut()
        .expect("Task manager not initialized")
        .create_task(name, entry)
}

/// Joins on a target task - blocks current task until target exits.
pub fn join(target: TaskRef) {
    let curr_task = percpu::current_task();

    TASK_MANAGER
        .lock()
        .as_mut()
        .expect("Task manager not initialized")
        .join_task(&curr_task, &target);
}

/// Puts the current task to sleep for the specified duration in nanoseconds.
pub fn sleep(duration_ns: u64) {
    let curr_task = percpu::current_task();
    let deadline_ns = current_nanoseconds() + duration_ns;

    TASK_MANAGER
        .lock()
        .as_mut()
        .expect("Task manager not initialized")
        .sleep_current(&curr_task, deadline_ns);
}

/// Current task yields CPU.
pub fn yield_now() {
    let curr_task = percpu::current_task();

    TASK_MANAGER
        .lock()
        .as_mut()
        .expect("Task manager not initialized")
        .yield_current(&curr_task);
}

/// Exits the current task.
pub fn exit_current() -> ! {
    let curr_task = percpu::current_task();

    TASK_MANAGER
        .lock()
        .as_mut()
        .expect("Task manager not initialized")
        .exit_current(&curr_task);
}

/// Called from timer interrupt.
pub fn on_timer_tick() {
    if let Some(manager) = TASK_MANAGER.lock().as_mut() {
        manager.timer_tick();
    }
}

/// Starts the scheduler. Never returns.
pub fn start_scheduling() -> ! {
    TASK_MANAGER
        .lock()
        .as_mut()
        .expect("Task manager not initialized")
        .start()
}
