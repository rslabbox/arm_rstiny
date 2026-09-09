//! Local CPU instruction and register operations.
use aarch64_cpu::{
    asm::barrier,
    registers::{DAIF, Readable},
};
use core::arch::asm;

#[inline]
pub fn flush_tlb_all() {
    unsafe { asm!("tlbi vmalle1; dsb sy; isb") };
}

/// Whether PSTATE.I masks IRQ exceptions on this CPU.
pub fn irq_masked() -> bool {
    DAIF.is_set(DAIF::I)
}

/// Wait with local IRQ exceptions masked. A pending enabled IRQ wakes WFI;
/// the caller must service it and recheck runnable work after returning.
/// Call with no shared-state borrow or continuation switch in progress.
pub(crate) fn wait_for_interrupt() {
    assert!(irq_masked());
    barrier::dsb(barrier::SY);
    aarch64_cpu::asm::wfi();
}
