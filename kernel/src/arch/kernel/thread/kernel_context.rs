//! AAPCS64 continuation switching between IRQ-masked kernel stacks.
use core::mem::{align_of, offset_of, size_of};

/// Registers preserved across a kernel function call. User exception state is
/// owned separately by UserContext. FP/SIMD and kernel TLS are not supported;
/// both sides of a switch already use the kernel's empty TTBR0 root.
#[repr(C)]
#[derive(Debug, Default)]
pub(crate) struct KernelContext {
    x19: u64,
    x20: u64,
    x21: u64,
    x22: u64,
    x23: u64,
    x24: u64,
    x25: u64,
    x26: u64,
    x27: u64,
    x28: u64,
    fp: u64, // x29: frame pointer
    lr: u64, // x30: continuation return address
    sp: u64,
}
impl KernelContext {
    pub fn entry(
        stack_top: usize,
        trampoline: unsafe extern "C" fn() -> !,
        argument: usize,
    ) -> Self {
        // AAPCS64 requires the execution stack, not this record, to align to 16.
        assert_eq!(stack_top & 15, 0);
        Self {
            x19: argument as u64,
            lr: trampoline as *const () as u64,
            sp: stack_top as u64,
            ..Self::default()
        }
    }
}

// STP/LDP access each named pair as two adjacent words. All assembly offsets
// below come from the fields; these checks protect the paired second operands.
const _: () = {
    assert!(offset_of!(KernelContext, x20) == offset_of!(KernelContext, x19) + 8);
    assert!(offset_of!(KernelContext, x22) == offset_of!(KernelContext, x21) + 8);
    assert!(offset_of!(KernelContext, x24) == offset_of!(KernelContext, x23) + 8);
    assert!(offset_of!(KernelContext, x26) == offset_of!(KernelContext, x25) + 8);
    assert!(offset_of!(KernelContext, x28) == offset_of!(KernelContext, x27) + 8);
    assert!(offset_of!(KernelContext, lr) == offset_of!(KernelContext, fp) + 8);
    assert!(size_of::<KernelContext>() == 13 * size_of::<u64>());
    assert!(align_of::<KernelContext>() == align_of::<u64>());
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
        // Save the caller's continuation before borrowing a scratch register.
        "stp x19, x20, [x0, #{x19}]",
        "stp x21, x22, [x0, #{x21}]",
        "stp x23, x24, [x0, #{x23}]",
        "stp x25, x26, [x0, #{x25}]",
        "stp x27, x28, [x0, #{x27}]",
        "stp x29, x30, [x0, #{fp}]",
        "mov x9, sp",
        "str x9, [x0, #{sp}]",

        // Restore a suspended call, or enter a new task via its trampoline.
        "ldp x19, x20, [x1, #{x19}]",
        "ldp x21, x22, [x1, #{x21}]",
        "ldp x23, x24, [x1, #{x23}]",
        "ldp x25, x26, [x1, #{x25}]",
        "ldp x27, x28, [x1, #{x27}]",
        "ldp x29, x30, [x1, #{fp}]",
        "ldr x9, [x1, #{sp}]",
        "mov sp, x9",
        "ret",
        x19 = const offset_of!(KernelContext, x19),
        x21 = const offset_of!(KernelContext, x21),
        x23 = const offset_of!(KernelContext, x23),
        x25 = const offset_of!(KernelContext, x25),
        x27 = const offset_of!(KernelContext, x27),
        fp = const offset_of!(KernelContext, fp),
        sp = const offset_of!(KernelContext, sp),
    );
}
