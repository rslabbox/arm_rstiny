//! Task scheduler implementation.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::context;
use super::task::{TaskContext, TaskControlBlock, TaskState};
use crate::config::kernel::DEFAULT_TIME_SLICE_MS;

pub type TaskRef = Arc<Mutex<TaskControlBlock>>;

/// Task scheduler
pub struct Scheduler {
    ready_bitmap: u32,
    ready_queues: [VecDeque<TaskRef>; 32],
    current_task: Option<TaskRef>,
    need_resched: AtomicBool, // Whether rescheduling is needed
}

impl Scheduler {
    /// Create a new scheduler
    pub const fn new() -> Self {
        const EMPTY_QUEUE: VecDeque<TaskRef> = VecDeque::new();
        Self {
            ready_bitmap: 0,
            ready_queues: [EMPTY_QUEUE; 32],
            current_task: None,
            need_resched: AtomicBool::new(false),
        }
    }

    /// Add a task to the ready queue
    pub fn add_task(&mut self, task: TaskRef) {
        let priority = task.lock().priority;
        self.ready_queues[priority as usize].push_back(task);
        self.ready_bitmap |= 1 << priority;
        self.need_resched.store(true, Ordering::Release);
    }

    /// Get the current running task
    pub fn current(&self) -> Option<TaskRef> {
        self.current_task.clone()
    }

    /// Find the highest priority with ready tasks
    fn find_highest_priority(&self) -> Option<u8> {
        if self.ready_bitmap == 0 {
            return None;
        }

        let clz: u32;
        unsafe {
            asm!(
                "clz {clz:w}, {bitmap:w}",
                clz = out(reg) clz,
                bitmap = in(reg) self.ready_bitmap,
            );
        }

        Some(31 - clz as u8)
    }

    /// Pick the next task to run
    fn pick_next_task(&mut self) -> Option<TaskRef> {
        let priority = self.find_highest_priority()?;
        let queue = &mut self.ready_queues[priority as usize];
        let task = queue.pop_front()?;

        // If the queue is now empty, clear the bitmap bit
        if queue.is_empty() {
            self.ready_bitmap &= !(1 << priority);
        }

        Some(task)
    }

    /// Perform task scheduling (context switch)
    pub fn schedule(&mut self) {
        // If no rescheduling is needed, return
        if !self.need_resched.load(Ordering::Acquire) {
            return;
        }

        self.need_resched.store(false, Ordering::Release);

        // Get the next task to run
        let next_task = match self.pick_next_task() {
            Some(task) => task,
            None => {
                // No ready tasks, keep current task running or halt
                return;
            }
        };

        let next_tid = next_task.lock().tid;
        debug!("Scheduling task {}", next_tid);

        // If there's a current task, save it back to ready queue
        let prev_task = self.current_task.take();
        if let Some(ref prev) = prev_task {
            let mut prev_tcb = prev.lock();
            if prev_tcb.state == TaskState::Running {
                prev_tcb.state = TaskState::Ready;
                prev_tcb.reset_time_slice(DEFAULT_TIME_SLICE_MS);
                drop(prev_tcb);
                self.add_task(prev.clone());
            }
        }

        // Set next task as current and switch to it
        {
            let mut next_tcb = next_task.lock();
            next_tcb.state = TaskState::Running;
            next_tcb.reset_time_slice(DEFAULT_TIME_SLICE_MS);
        }

        self.current_task = Some(next_task.clone());

        // Perform context switch
        // We need to get contexts without holding locks during the switch
        let next_sp = next_task.lock().context.sp;

        if let Some(ref prev) = prev_task {
            let mut prev_tcb = prev.lock();
            let prev_ctx = &mut prev_tcb.context;
            unsafe {
                context::switch_to(Some(prev_ctx), &TaskContext { sp: next_sp });
            }
        } else {
            debug!("First context switch to task {}", next_tid);
            unsafe {
                context::switch_to(None, &TaskContext { sp: next_sp });
            }
        }
    }

    /// Current task yields the CPU
    pub fn yield_now(&mut self) {
        self.need_resched.store(true, Ordering::Release);
        self.schedule();
    }

    /// Current task exits
    pub fn exit(&mut self) {
        if let Some(current) = self.current_task.take() {
            let mut tcb = current.lock();
            tcb.state = TaskState::Exited;
            drop(tcb);
            // Don't add back to ready queue
        }

        self.need_resched.store(true, Ordering::Release);
        self.schedule();
    }

    /// Handle timer tick (time slice countdown)
    pub fn tick(&mut self) {
        use core::sync::atomic::AtomicUsize;
        static TICK_COUNT: AtomicUsize = AtomicUsize::new(0);
        
        let count = TICK_COUNT.fetch_add(1, Ordering::Relaxed);
        if count % 100 == 0 {
            debug!("Timer tick count: {}, ready_bitmap: {:#x}", count, self.ready_bitmap);
        }
        
        if let Some(ref current) = self.current_task {
            let mut tcb = current.lock();
            if tcb.time_slice > 0 {
                tcb.time_slice -= 1;
            }

            if tcb.time_slice == 0 {
                // Time slice exhausted
                self.need_resched.store(true, Ordering::Release);
            }
        } else {
            // No current task, but check if there are ready tasks
            if self.ready_bitmap != 0 {
                debug!("No current task, but ready_bitmap = {:#x}, triggering schedule", self.ready_bitmap);
                self.need_resched.store(true, Ordering::Release);
            }
        }

        // Check if we need to reschedule
        if self.need_resched.load(Ordering::Acquire) {
            self.schedule();
        }
    }
}

/// Global scheduler instance
static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

/// Add a task to the scheduler
pub fn add_task(task: TaskRef) {
    SCHEDULER.lock().add_task(task);
}

/// Current task yields the CPU
pub fn yield_now() {
    SCHEDULER.lock().yield_now();
}

/// Current task exits
pub fn exit() -> ! {
    SCHEDULER.lock().exit();
    unreachable!("Task should have been switched out");
}

/// Handle timer tick
pub fn tick() {
    SCHEDULER.lock().tick();
}

/// Get the current running task
pub fn current_task() -> Option<TaskRef> {
    SCHEDULER.lock().current()
}
