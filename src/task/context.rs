//! Task context switching interface.

use super::task::TaskContext;

core::arch::global_asm!(include_str!("switch.S"));

/// Perform context switch
///
/// # Safety
/// This function directly manipulates stack pointers and must ensure
/// that the passed contexts are valid.
pub unsafe fn switch_to(current: Option<&mut TaskContext>, next: &TaskContext) {
    unsafe extern "C" {
        fn __switch_to(current_sp: *mut usize, next_sp: usize);
    }

    if let Some(current_ctx) = current {
        // Save current context and switch to next
        unsafe {
            __switch_to(&mut current_ctx.sp as *mut usize, next.sp);
        }
    } else {
        // First switch, no need to save current context
        unsafe {
            __switch_to(core::ptr::null_mut(), next.sp);
        }
    }
}
