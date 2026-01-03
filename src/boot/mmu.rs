//! MMU (Memory Management Unit) initialization.

use aarch64_cpu::asm::barrier;
use aarch64_cpu::registers::*;
use memory_addr::PhysAddr;

/// Configures and enables the MMU on the current CPU.
///
/// This function sets up separate page tables for TTBR0 (identity mapping)
/// and TTBR1 (kernel high address mapping) to support position-independent
/// kernel loading.
///
/// # Arguments
/// * `ttbr0_paddr` - Physical address of the L0 page table for identity mapping
/// * `ttbr1_paddr` - Physical address of the L0 page table for kernel mapping
///
/// # Safety
///
/// This function is unsafe as it changes the address translation configuration.
#[unsafe(no_mangle)]
pub unsafe fn init_mmu(ttbr0_paddr: PhysAddr, ttbr1_paddr: PhysAddr) {
    use page_table_entry::aarch64::MemAttr;

    MAIR_EL1.set(MemAttr::MAIR_VALUE);

    // Enable TTBR0 and TTBR1 walks, page size = 4K, vaddr size = 48 bits, paddr size = 48 bits.
    let tcr_flags0 = TCR_EL1::EPD0::EnableTTBR0Walks
        + TCR_EL1::TG0::KiB_4
        + TCR_EL1::SH0::Inner
        + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::T0SZ.val(16);
    let tcr_flags1 = TCR_EL1::EPD1::EnableTTBR1Walks
        + TCR_EL1::TG1::KiB_4
        + TCR_EL1::SH1::Inner
        + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::T1SZ.val(16);
    TCR_EL1.write(TCR_EL1::IPS::Bits_48 + tcr_flags0 + tcr_flags1);
    barrier::isb(barrier::SY);

    // Set TTBR0 for identity mapping (low addresses, used during transition)
    // Set TTBR1 for kernel mapping (high addresses, 0xffff_xxxx_xxxx_xxxx)
    TTBR0_EL1.set(ttbr0_paddr.as_usize() as u64);
    TTBR1_EL1.set(ttbr1_paddr.as_usize() as u64);

    // Flush the entire TLB
    crate::hal::flush_tlb(None);

    // Enable the MMU and turn on I-cache and D-cache
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);
    barrier::isb(barrier::SY);
}
