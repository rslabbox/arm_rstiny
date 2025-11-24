//! Context switch assembly implementation.

.section .text
.global __switch_to
.type __switch_to, @function

/// Context switch function
/// 
/// Arguments:
///   x0 = current_sp address (pointer to save current SP, can be null)
///   x1 = next_sp value (new stack pointer to load)
///
/// This function only switches the stack pointer.
/// Register saving/restoration is handled by the interrupt handler's
/// SAVE_REGS/RESTORE_REGS macros.
__switch_to:
    // If current_sp is null (first switch), skip saving
    cbz     x0, 1f
    
    // Save current stack pointer to current_sp
    mov     x2, sp
    str     x2, [x0]
    
1:
    // Load new stack pointer
    mov     sp, x1
    ret

.size __switch_to, . - __switch_to
