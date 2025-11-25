//! Task scheduler with round-robin time-slice scheduling.

use alloc::collections::{BinaryHeap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::drivers::timer::{current_ticks, ticks_to_nanos};
use crate::hal::TrapFrame;

use super::task::{Task, TaskId, TaskState};

/// Default time slice in ticks (10 ticks = 10ms at 1000 Hz).
pub const DEFAULT_TIME_SLICE: usize = 10;

/// Default task stack size (64 KB).
pub const DEFAULT_STACK_SIZE: usize = 0x10000;

/// Sleeping task entry for priority queue.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct SleepingTask {
    tid: usize,
    wakeup_time: u64,
}

impl Ord for SleepingTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering: earlier wakeup time has higher priority
        other.wakeup_time.cmp(&self.wakeup_time)
    }
}

impl PartialOrd for SleepingTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Round-robin scheduler.
pub struct Scheduler {
    /// All tasks.
    tasks: Vec<Task>,
    /// Index of the currently running task.
    current: Option<usize>,
    /// Ready queue (indices into tasks vec).
    ready_queue: VecDeque<usize>,
    /// Sleeping tasks (min-heap by wakeup time).
    sleep_queue: BinaryHeap<SleepingTask>,
    /// Next task ID to assign.
    next_tid: usize,
}

impl Scheduler {
    /// Create a new scheduler.
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            current: None,
            ready_queue: VecDeque::new(),
            sleep_queue: BinaryHeap::new(),
            next_tid: 0,
        }
    }

    /// Spawn a new task.
    ///
    /// # Arguments
    /// * `name` - Task name for debugging
    /// * `entry` - Task entry point address
    /// * `arg` - Argument to pass to the task
    ///
    /// # Returns
    /// The new task's ID.
    pub fn spawn(&mut self, name: String, entry: usize, arg: usize) -> TaskId {
        let tid = self.next_tid;
        self.next_tid += 1;

        let mut task = Task::new(tid, name, entry, arg, DEFAULT_STACK_SIZE);
        task.time_slice = DEFAULT_TIME_SLICE;

        info!("Spawning task {}: {}", tid, task.name);

        self.tasks.push(task);
        self.ready_queue.push_back(tid);

        tid
    }

    /// Handle timer tick - decrement time slice and wake up sleeping tasks.
    pub fn tick(&mut self) {
        // Decrement current task's time slice
        if let Some(idx) = self.current {
            if self.tasks[idx].time_slice > 0 {
                self.tasks[idx].time_slice -= 1;
            }
        }

        // Wake up sleeping tasks
        let current_time = ticks_to_nanos(current_ticks());
        self.wakeup_tasks(current_time);
    }

    /// Perform task scheduling and switch context if needed.
    ///
    /// # Arguments
    /// * `tf` - Trap frame from interrupt handler
    pub fn schedule(&mut self, tf: &mut TrapFrame) {
        // Save current task context
        if let Some(idx) = self.current {
            let task = &mut self.tasks[idx];

            // Only save and reschedule if still running
            if task.state == TaskState::Running {
                task.context = super::context::TaskContext::from_trap_frame(tf);

                // If time slice expired, move to ready queue
                if task.time_slice == 0 {
                    task.state = TaskState::Ready;
                    task.time_slice = DEFAULT_TIME_SLICE;
                    self.ready_queue.push_back(idx);
                    self.current = None;
                } else {
                    // Still has time slice, continue running
                    return;
                }
            } else if task.state == TaskState::Dead {
                // Task finished, remove from current
                debug!("Current task {} is Dead, clearing current", task.id);
                self.current = None;
            }
        }

        // Select next task from ready queue
        while let Some(next_idx) = self.ready_queue.pop_front() {
            if next_idx < self.tasks.len() && self.tasks[next_idx].state == TaskState::Ready {
                self.switch_to(next_idx, tf);
                return;
            }
        }

        // No ready task - return without modifying TrapFrame
        // This allows returning to the interrupted context (main thread or sleeping task)
        if self.current.is_none() {
            trace!("No ready tasks and no current task, returning to interrupted context");
        }
    }

    /// Yield the current task voluntarily.
    pub fn yield_current(&mut self, tf: &mut TrapFrame) {
        if let Some(idx) = self.current {
            let task = &mut self.tasks[idx];
            task.context = super::context::TaskContext::from_trap_frame(tf);
            task.state = TaskState::Ready;
            task.time_slice = DEFAULT_TIME_SLICE; // Reset time slice
            self.ready_queue.push_back(idx);
            self.current = None;
        }

        self.schedule(tf);
    }

    /// Put the current task to sleep.
    ///
    /// # Arguments
    /// * `duration_ns` - Sleep duration in nanoseconds
    /// * `tf` - Trap frame from interrupt handler
    pub fn sleep_current(&mut self, duration_ns: u64, tf: &mut TrapFrame) {
        if let Some(idx) = self.current {
            let current_time = ticks_to_nanos(current_ticks());
            let wakeup_time = current_time + duration_ns;

            let task = &mut self.tasks[idx];
            task.context = super::context::TaskContext::from_trap_frame(tf);
            task.state = TaskState::Blocked;
            task.wakeup_time = Some(wakeup_time);

            debug!("Task {} sleeping until {} ns", task.id, wakeup_time);

            self.sleep_queue.push(SleepingTask {
                tid: idx,
                wakeup_time,
            });

            self.current = None;
        }

        self.schedule(tf);
    }

    /// Get the current task ID.
    pub fn current_id(&self) -> Option<TaskId> {
        self.current.map(|idx| self.tasks[idx].id)
    }

    /// Get the next task ID (for naming purposes).
    pub fn next_task_id(&self) -> usize {
        self.next_tid
    }

    /// Get a task by its ID.
    pub fn get_task(&self, task_id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == task_id)
    }

    /// Mark the current task as dead (called when task exits).
    pub fn exit_current(&mut self) {
        if let Some(idx) = self.current {
            let task = &mut self.tasks[idx];
            info!("Task {} ({}) exiting", task.id, task.name);
            task.state = TaskState::Dead;
            // Don't add to ready queue - dead tasks should not run
            self.current = None;
        }
    }

    /// Wake up tasks whose sleep time has expired.
    fn wakeup_tasks(&mut self, current_time: u64) {
        while let Some(sleeping) = self.sleep_queue.peek() {
            if sleeping.wakeup_time <= current_time {
                let sleeping = self.sleep_queue.pop().unwrap();
                let idx = sleeping.tid;

                if idx < self.tasks.len() {
                    let task = &mut self.tasks[idx];
                    if task.state == TaskState::Blocked && task.should_wakeup(current_time) {
                        debug!("Waking up task {}", task.id);
                        task.state = TaskState::Ready;
                        task.wakeup_time = None;
                        task.time_slice = DEFAULT_TIME_SLICE;
                        self.ready_queue.push_back(idx);
                    }
                }
            } else {
                break;
            }
        }
    }

    /// Switch to the specified task.
    fn switch_to(&mut self, next_idx: usize, tf: &mut TrapFrame) {
        let task = &mut self.tasks[next_idx];
        task.state = TaskState::Running;
        task.context.to_trap_frame(tf);
        self.current = Some(next_idx);
    }

    /// Dump all tasks for debugging.
    #[allow(dead_code)]
    pub fn dump_tasks(&self) {
        info!("=== Task List ===");
        for task in &self.tasks {
            info!("{:?}", task);
        }
        info!("Current: {:?}", self.current);
        info!("Ready queue: {:?}", self.ready_queue);
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}
