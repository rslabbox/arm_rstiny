//! Temporary EL1 mappings for the physical loader and high-address kernel entry.
//! TTBR0 keeps the loader identity map. TTBR1 contains the physical direct map
//! and a separate kernel image mapping to its dynamically allocated RAM.
mod tables;
use aarch64_cpu::{asm::barrier, registers::*};
use core::{arch::asm, ptr::addr_of};
pub(super) use tables::init_boot_page_tables;
use tables::{HIGH_ROOT, ROOT};

/// # Safety
/// Enter at EL1 with MMU/caches off, IRQs masked, and initialized boot tables.
/// Code, stack, tables and loaded images must lie in the identity-mapped RAM.
/// Firmware must have left no dirty cache state needing preservation.
pub unsafe fn enable_mmu() {
    // Publish table/image writes and discard stale instruction cache contents
    // before executing code copied into RAM by the loader.
    barrier::dsb(barrier::SY);
    // SAFETY: Privileged EL1 cache maintenance under the entry contract.
    unsafe {
        asm!("ic iallu", options(nostack));
    }
    barrier::dsb(barrier::SY);
    barrier::isb(barrier::SY);

    // Preserve the seL4 loader's MAIR layout; the kernel retains these slots
    // while switching its live mappings. Slots 0 and 4 are used by our tree.
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_noEarlyWriteAck
            + MAIR_EL1::Attr1_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr2_Device::Gathering_Reordering_EarlyWriteAck
            + MAIR_EL1::Attr3_Normal_Outer::NonCacheable
            + MAIR_EL1::Attr3_Normal_Inner::NonCacheable
            + MAIR_EL1::Attr4_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr4_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr5_Normal_Outer::WriteThrough_NonTransient_ReadAlloc
            + MAIR_EL1::Attr5_Normal_Inner::WriteThrough_NonTransient_ReadAlloc,
    );
    // 48-bit low/high VA spaces, 4 KiB granules, coherent WB table walks.
    // Cortex-A72 supports the 16-bit ASIDs used by the existing boot contract.
    TCR_EL1.write(
        TCR_EL1::T0SZ.val(16)
            + TCR_EL1::T1SZ.val(16)
            + TCR_EL1::TG0::KiB_4
            + TCR_EL1::TG1::KiB_4
            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::SH0::Inner
            + TCR_EL1::SH1::Inner
            + TCR_EL1::IPS.val(ID_AA64MMFR0_EL1.read(ID_AA64MMFR0_EL1::PARange))
            + TCR_EL1::AS::ASID16Bits,
    );
    let root = addr_of!(ROOT) as u64;
    TTBR0_EL1.set(root);
    TTBR1_EL1.set(addr_of!(HIGH_ROOT) as u64);
    barrier::isb(barrier::SY);
    // SAFETY: No user contexts exist; discard every stale EL1 translation.
    unsafe {
        asm!("tlbi vmalle1", options(nostack));
    }
    barrier::dsb(barrier::SY);
    barrier::isb(barrier::SY);

    // Identity mapping keeps the current PC and SP valid across this write.
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);
    barrier::isb(barrier::SY);
}
