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

/// Get the D-cache line size from CTR_EL0 register
#[inline]
fn get_dcache_line_size() -> usize {
    let ctr: usize;
    unsafe {
        asm!("mrs {}, ctr_el0", out(reg) ctr);
    }
    // DminLine is bits [19:16], log2 of the number of words (4 bytes)
    let dminline = (ctr >> 16) & 0xF;
    4 << dminline // Convert log2(words) to bytes
}

/// Clean (write-back) data cache by virtual address range
///
/// This operation writes modified cache lines back to memory but leaves them in the cache.
/// This is required before DMA operations that read from memory (CPU -> Device).
///
/// # Safety
/// The caller must ensure that the address range is valid and properly aligned.
#[inline]
pub unsafe fn clean_dcache_range(addr: usize, size: usize) {
    if size == 0 {
        return;
    }

    let cache_line_size = get_dcache_line_size();
    let start = addr & !(cache_line_size - 1);
    let end = (addr + size + cache_line_size - 1) & !(cache_line_size - 1);

    let mut current = start;
    while current < end {
        unsafe {
            // DC CVAC - Data Cache Clean by VA to Point of Coherency
            asm!("dc cvac, {}", in(reg) current);
        }
        current += cache_line_size;
    }

    unsafe {
        // Ensure completion and visibility
        asm!("dsb sy");
    }
}

/// Invalidate (discard) data cache by virtual address range
///
/// This operation discards cache lines, forcing subsequent reads to fetch from memory.
/// This is required after DMA operations that write to memory (Device -> CPU).
///
/// # Safety
/// The caller must ensure that the address range is valid and properly aligned.
/// Invalidating cache lines with dirty data can cause data loss.
#[inline]
pub unsafe fn invalidate_dcache_range(addr: usize, size: usize) {
    if size == 0 {
        return;
    }

    let cache_line_size = get_dcache_line_size();
    let start = addr & !(cache_line_size - 1);
    let end = (addr + size + cache_line_size - 1) & !(cache_line_size - 1);

    let mut current = start;
    while current < end {
        unsafe {
            // DC IVAC - Data Cache Invalidate by VA to Point of Coherency
            asm!("dc ivac, {}", in(reg) current);
        }
        current += cache_line_size;
    }

    unsafe {
        // Ensure completion
        asm!("dsb sy");
    }
}
