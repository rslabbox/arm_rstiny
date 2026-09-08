//! A returning, IRQ-masked boundary around one interval of EL0 execution.
use super::{TrapFrame, irq};
use crate::memory;
use core::mem::{offset_of, size_of};

pub struct UserContext(TrapFrame);
impl UserContext {
    pub fn new(frame: TrapFrame) -> Self {
        Self(frame)
    }
    pub fn frame(&self) -> &TrapFrame {
        &self.0
    }
    pub fn frame_mut(&mut self) -> &mut TrapFrame {
        &mut self.0
    }

    /// # Safety
    /// The page-table root and all mappings must remain owned for this call.
    /// Only the single-CPU runtime may call this, without shared-state borrows.
    pub unsafe fn run(&mut self, root: usize) -> UserEvent {
        assert!(irq::masked());
        let mut trap = RawTrap::default();
        memory::activate(root);
        // SAFETY: exclusively borrowed context and stack-local result remain
        // alive until the assembly restores this kernel continuation.
        unsafe { run_user(&mut self.0, &mut trap) };
        memory::activate_kernel();
        assert!(irq::masked());
        match trap.kind {
            1 => UserEvent::Interrupt,
            0 if trap.esr >> 26 == 0x15 && trap.esr & 0xffff == 0 => UserEvent::Syscall,
            0 => UserEvent::Fault(UserFault {
                esr: trap.esr,
                far: match (trap.esr >> 26, trap.esr & (1 << 10)) {
                    (0x20 | 0x24, 0) => Some(trap.far),
                    _ => None,
                },
            }),
            _ => unreachable!("fatal architecture events cannot return"),
        }
    }
}
#[derive(Default)]
#[repr(C)]
pub(super) struct RawTrap {
    pub kind: u64,
    pub esr: u64,
    pub far: u64,
}
pub struct UserFault {
    pub esr: u64,
    pub far: Option<u64>,
}
pub enum UserEvent {
    Syscall,
    Interrupt,
    Fault(UserFault),
}

// AAPCS64 callee-saved registers plus trusted pointers, never user-visible.
#[repr(C, align(16))]
pub(super) struct KernelReturnFrame {
    pub saved: [u64; 12],
    pub context: *mut TrapFrame,
    pub trap: *mut RawTrap,
}
const _: () = {
    assert!(size_of::<KernelReturnFrame>() == 112);
    assert!(offset_of!(KernelReturnFrame, context) == 96);
    assert!(offset_of!(KernelReturnFrame, trap) == 104);
    assert!(size_of::<RawTrap>() == 24);
};

#[unsafe(naked)]
#[unsafe(export_name = "run_user")]
unsafe extern "C" fn run_user(context: *mut TrapFrame, trap: *mut RawTrap) {
    core::arch::naked_asm!(
        "sub sp, sp, #{return_size}",
        "stp x19, x20, [sp, #0]", "stp x21, x22, [sp, #16]",
        "stp x23, x24, [sp, #32]", "stp x25, x26, [sp, #48]",
        "stp x27, x28, [sp, #64]", "stp x29, x30, [sp, #80]",
        "stp x0, x1, [sp, #{context_offset}]",
        "mov x30, x0",
        "ldp x9, x10, [x30, #{usp}]",
        "ldr x11, [x30, #{spsr}]",
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
        return_size = const size_of::<KernelReturnFrame>(),
        context_offset = const offset_of!(KernelReturnFrame, context),
        usp = const offset_of!(TrapFrame, usp),
        spsr = const offset_of!(TrapFrame, spsr),
    );
}
