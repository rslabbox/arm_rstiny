//! Task context switching.

use crate::hal::TrapFrame;

/// Perform task context switch.
///
/// This function is called during scheduling to switch from one task to another.
/// The actual context switching is handled by the scheduler loading the new
/// TrapFrame into the current context, which will be restored by the exception
/// return mechanism.
///
/// # Arguments
/// * `current_ctx` - Current task's context (will be saved)
/// * `next_ctx` - Next task's context (will be loaded)
#[inline]
pub fn task_context_switch(current_ctx: &mut TrapFrame, next_ctx: &TrapFrame) {
    *current_ctx = *next_ctx;
}
