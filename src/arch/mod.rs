use core::arch::asm;
use memory_addr::VirtAddr;
pub mod boot;
pub mod dw_apb_uart;
pub mod exception;
pub mod gicv3;
pub mod mem;
pub mod psci;

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

pub fn arch_init() {
    exception::init_trap();
    mem::clear_bss();
    gicv3::irq_init();
}
