//! AAPCS64 continuation switching between IRQ-masked kernel stacks.
use core::mem::{offset_of, size_of};

#[repr(C, align(16))]
pub(crate) struct KernelContext {
    saved: [u64; 12], // x19..x30
    sp: u64,
}
impl KernelContext {
    pub const fn empty() -> Self {
        Self {
            saved: [0; 12],
            sp: 0,
        }
    }
    pub fn entry(stack_top: usize, entry: usize, argument: usize) -> Self {
        assert_eq!(stack_top & 15, 0);
        let mut context = Self::empty();
        context.saved[0] = argument as u64; // trampoline's x19
        context.saved[11] = entry as u64; // first return goes to the trampoline
        context.sp = stack_top as u64;
        context
    }
}
const _: () = {
    assert!(offset_of!(KernelContext, sp) == 96);
    assert!(size_of::<KernelContext>() == 112);
};

/// # Safety
/// Both contexts and their stacks must be exclusively owned and live until the
/// switch completes. No shared-state guards may cross this call. IRQs must be
/// masked and TTBR0 must be the kernel's empty root on both sides.
/// FP/SIMD is disabled; TLS and SMP are not supported.
#[unsafe(naked)]
#[unsafe(export_name = "switch_kernel_context")]
pub(crate) unsafe extern "C" fn switch(
    outgoing: *mut KernelContext,
    incoming: *const KernelContext,
) {
    core::arch::naked_asm!(
        // Save the caller's continuation before borrowing scratch registers.
        "stp x19, x20, [x0, #0]", "stp x21, x22, [x0, #16]",
        "stp x23, x24, [x0, #32]", "stp x25, x26, [x0, #48]",
        "stp x27, x28, [x0, #64]", "stp x29, x30, [x0, #80]",
        "mov x9, sp", "str x9, [x0, #{sp}]",
        // Resume a saved call, or enter a new task through its trampoline.
        "ldp x19, x20, [x1, #0]", "ldp x21, x22, [x1, #16]",
        "ldp x23, x24, [x1, #32]", "ldp x25, x26, [x1, #48]",
        "ldp x27, x28, [x1, #64]", "ldp x29, x30, [x1, #80]",
        "ldr x9, [x1, #{sp}]", "mov sp, x9", "ret",
        sp = const offset_of!(KernelContext, sp),
    );
}
