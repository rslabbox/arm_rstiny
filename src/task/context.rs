//! Task context for saving and restoring CPU state.

use crate::hal::TrapFrame;

/// Task execution context containing all CPU registers.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TaskContext {
    /// General-purpose registers (x0-x30).
    pub gpr: [u64; 31],
    /// Stack pointer.
    pub sp: u64,
    /// Program counter (entry point or resume address).
    pub pc: u64,
    /// Saved processor status register.
    pub spsr: u64,
}

impl TaskContext {
    /// Create a new task context for a fresh task.
    ///
    /// # Arguments
    /// * `entry` - Task entry point address
    /// * `arg` - Argument to pass to the task (in x0)
    /// * `stack_top` - Top of the task stack
    pub fn new(entry: usize, arg: usize, stack_top: usize) -> Self {
        let mut gpr = [0u64; 31];
        gpr[0] = arg as u64; // x0 = argument

        Self {
            gpr,
            sp: stack_top as u64,
            pc: entry as u64,
            // EL1h mode, all interrupts enabled
            // D=0, A=0, I=0, F=0 (interrupts enabled)
            // M=0b0101 (EL1h - using SP_EL1)
            spsr: 0x0000_0000_0000_0005,
        }
    }

    /// Create a context from a trap frame (when interrupted).
    pub fn from_trap_frame(tf: &TrapFrame) -> Self {
        Self {
            gpr: tf.r,
            sp: tf.usp,
            pc: tf.elr,
            spsr: tf.spsr,
        }
    }

    /// Restore this context to a trap frame (for switching).
    pub fn to_trap_frame(&self, tf: &mut TrapFrame) {
        tf.r = self.gpr;
        tf.usp = self.sp;
        tf.elr = self.pc;
        tf.spsr = self.spsr;
    }
}

impl Default for TaskContext {
    fn default() -> Self {
        Self {
            gpr: [0; 31],
            sp: 0,
            pc: 0,
            spsr: 0x0000_0000_0000_0005,
        }
    }
}
