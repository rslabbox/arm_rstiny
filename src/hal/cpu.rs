//! CPU-related operations and utilities.

use aarch64_cpu::registers::{DAIF, TPIDR_EL1};
use core::arch::asm;
use memory_addr::VirtAddr;
use tock_registers::interfaces::{Readable, Writeable};

/// Gets the current CPU's thread pointer (TPIDR_EL1).
///
/// This is used to store a pointer to the per-CPU data structure.
#[inline]
pub fn thread_pointer() -> usize {
    TPIDR_EL1.get() as _
}

/// Sets the current CPU's thread pointer (TPIDR_EL1).
///
/// # Safety
///
/// The caller must ensure that `tp` points to a valid PerCpu structure.
#[inline]
pub unsafe fn set_thread_pointer(tp: usize) {
    TPIDR_EL1.set(tp as _)
}

/// Flushes the TLB.
///
/// If `vaddr` is [`None`], flushes the entire TLB. Otherwise, flushes the TLB
/// entry that maps the given virtual address.
#[inline]
pub fn flush_tlb(vaddr: Option<VirtAddr>) {
    if let Some(vaddr) = vaddr {
        const VA_MASK: usize = (1 << 44) - 1; // VA[55:12] => bits[43:0]
        let operand = (vaddr.as_usize() >> 12) & VA_MASK;

        unsafe {
            // TLB Invalidate by VA, All ASID, EL1, Inner Shareable
            asm!("tlbi vaae1is, {}; dsb sy; isb", in(reg) operand)
        }
    } else {
        // flush the entire TLB
        unsafe {
            // TLB Invalidate by VMID, All at stage 1, EL1
            asm!("tlbi vmalle1; dsb sy; isb")
        }
    }
}

/// Fills the `.bss` section with zeros.
///
/// It requires the symbols `_sbss` and `_ebss` to be defined in the linker script.
///
/// # Safety
/// This function is unsafe because it writes `.bss` section directly.
pub fn clear_bss() {
    unsafe extern "C" {
        fn _sbss();
        fn _ebss();
    }

    unsafe {
        core::slice::from_raw_parts_mut(
            _sbss as *const () as usize as *mut u8,
            _ebss as *const () as usize - _sbss as *const () as usize,
        )
        .fill(0);
    }
}


/// Enables interrupts.
///
/// This function clears the DAIF I bit to enable IRQ interrupts.
#[inline]
pub fn enable_irqs() {
    unsafe { asm!("msr daifclr, #2") };
}

/// Disables interrupts.
///
/// This function sets the DAIF I bit to disable IRQ interrupts.
#[inline]
pub fn disable_irqs() {
    unsafe { asm!("msr daifset, #2") };
}

/// Checks if interrupts are disabled.
///
/// Returns `true` if the DAIF I bit is set (masked).
#[inline]
pub fn irqs_disabled() -> bool {
    DAIF.matches_all(DAIF::I::Masked)
}
