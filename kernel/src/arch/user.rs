use super::TrapFrame;

/// Register restore is the unavoidable assembly boundary; all setup is Rust.
/// Reset the EL1 stack before eret, abandoning boot-time Rust call frames.
#[unsafe(naked)]
pub unsafe extern "C" fn enter(frame: *const TrapFrame) -> ! {
    core::arch::naked_asm!(
        "mov x30, x0",
        "ldr x9, =boot_stack_top",
        "mov sp, x9",
        "ldp x9, x10, [x30, #248]",
        "ldr x11, [x30, #264]",
        "msr sp_el0, x9",
        "msr elr_el1, x10",
        "msr spsr_el1, x11",
        "ldp x0, x1, [x30, #0]",
        "ldp x2, x3, [x30, #16]",
        "ldp x4, x5, [x30, #32]",
        "ldp x6, x7, [x30, #48]",
        "ldp x8, x9, [x30, #64]",
        "ldp x10, x11, [x30, #80]",
        "ldp x12, x13, [x30, #96]",
        "ldp x14, x15, [x30, #112]",
        "ldp x16, x17, [x30, #128]",
        "ldp x18, x19, [x30, #144]",
        "ldp x20, x21, [x30, #160]",
        "ldp x22, x23, [x30, #176]",
        "ldp x24, x25, [x30, #192]",
        "ldp x26, x27, [x30, #208]",
        "ldp x28, x29, [x30, #224]",
        "ldr x30, [x30, #240]",
        "eret",
    );
}
