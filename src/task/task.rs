//! Task Control Block (TCB) and related structures.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::hal::TrapFrame;

/// Task ID type
pub type TaskId = usize;

/// Task state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,   // Ready to run
    Running, // Currently running
    Exited,  // Exited
}

/// Task context (saved register state)
#[derive(Debug, Clone, Copy)]
pub struct TaskContext {
    pub sp: usize, // Stack pointer SP_EL1
}

impl TaskContext {
    pub const fn new() -> Self {
        Self { sp: 0 }
    }
}

/// Task Control Block
pub struct TaskControlBlock {
    pub tid: TaskId,
    pub priority: u8,
    pub state: TaskState,
    pub context: TaskContext,
    pub stack: Box<[u8]>,
    pub time_slice: u32, // Remaining time slice (ms)
    entry: usize,        // Task entry address (for initialization)
}

impl TaskControlBlock {
    /// Create a new task
    pub fn new(
        tid: TaskId,
        entry: usize,
        priority: u8,
        stack_size: usize,
        time_slice: u32,
    ) -> Self {
        let stack = alloc::vec![0u8; stack_size].into_boxed_slice();
        let mut tcb = Self {
            tid,
            priority,
            state: TaskState::Ready,
            context: TaskContext::new(),
            stack,
            time_slice,
            entry,
        };
        tcb.init_stack();
        tcb
    }

    /// Initialize task stack (construct initial TrapFrame)
    fn init_stack(&mut self) {
        let stack_top = self.stack.as_ptr() as usize + self.stack.len();

        // Align to 16 bytes (AArch64 requirement)
        let stack_top = stack_top & !0xf;

        // Reserve space for TrapFrame at the top of stack
        let trap_frame_ptr = (stack_top - core::mem::size_of::<TrapFrame>()) as *mut TrapFrame;

        unsafe {
            let trap_frame = &mut *trap_frame_ptr;

            // Initialize all registers to zero
            *trap_frame = TrapFrame::default();

            // Set entry point
            trap_frame.elr = self.entry as u64;

            // Set processor state: EL1h, interrupts enabled
            // SPSR_EL1: M[3:0]=0b0101 (EL1h), all interrupt masks cleared
            trap_frame.spsr = 0b0101; // EL1h mode

            // Set user stack pointer (not used for kernel tasks, but set it anyway)
            trap_frame.usp = stack_top as u64;

            // Set x0 to task ID (can be used as argument)
            trap_frame.r[0] = self.tid as u64;
        }

        // Set context stack pointer to point to the TrapFrame
        self.context.sp = trap_frame_ptr as usize;
    }

    /// Reset time slice
    pub fn reset_time_slice(&mut self, time_slice: u32) {
        self.time_slice = time_slice;
    }
}

/// Global task ID generator
static NEXT_TID: AtomicUsize = AtomicUsize::new(1);

/// Allocate a new task ID
pub fn alloc_tid() -> TaskId {
    NEXT_TID.fetch_add(1, Ordering::Relaxed)
}
