//! CPU-related operations and utilities.

use core::arch::asm;
use memory_addr::VirtAddr;

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
        core::slice::from_raw_parts_mut(_sbss as usize as *mut u8, _ebss as usize - _sbss as usize)
            .fill(0);
    }
}
