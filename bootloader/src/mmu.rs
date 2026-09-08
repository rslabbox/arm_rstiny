//! Temporary EL1 mappings for the physical loader and high-address kernel entry.
//! TTBR0 keeps the loader identity map. TTBR1 contains the physical direct map
//! and a separate kernel image mapping to its dynamically allocated RAM.
use aarch64_cpu::{asm::barrier, registers::*};
use core::{
    arch::asm,
    ptr::{addr_of, addr_of_mut},
};

bitflags::bitflags! {
    struct DescriptorAttr: u64 {
        const VALID = 1 << 0;
        const TABLE = 1 << 1;
        const NORMAL = 4 << 2; // MAIR slot 4; Device uses slot 0.
        const INNER_SHAREABLE = 3 << 8;
        const ACCESS = 1 << 10;
        const PXN = 1 << 53;
        const UXN = 1 << 54;
    }
}

#[derive(Clone, Copy)]
#[repr(transparent)]
struct PageTableEntry(u64);
#[repr(usize)]
enum BlockSize {
    GiB1 = 1 << 30,
    MiB2 = 1 << 21,
}
enum MemoryType {
    Device,
    Normal,
}
impl PageTableEntry {
    const EMPTY: Self = Self(0);
    fn table(address: usize) -> Self {
        assert!(address.is_multiple_of(4096));
        Self(address as u64 | (DescriptorAttr::VALID | DescriptorAttr::TABLE).bits())
    }
    /// L1/L2 block, privileged read/write. Device is XN; RAM permits EL1 execution.
    fn block(address: usize, memory: MemoryType, size: BlockSize) -> Self {
        assert!(address.is_multiple_of(size as usize));
        let attributes = DescriptorAttr::VALID
            | DescriptorAttr::ACCESS
            | DescriptorAttr::UXN
            | match memory {
                MemoryType::Device => DescriptorAttr::PXN,
                MemoryType::Normal => DescriptorAttr::NORMAL | DescriptorAttr::INNER_SHAREABLE,
            };
        Self(address as u64 | attributes.bits())
    }
}
#[repr(C, align(4096))]
struct Table([PageTableEntry; 512]);
impl Table {
    const EMPTY: Self = Self([PageTableEntry::EMPTY; 512]);
}
static mut ROOT: Table = Table::EMPTY;
static mut HIGH_ROOT: Table = Table::EMPTY;
static mut KERNEL_L1: Table = Table::EMPTY;
static mut KERNEL_L2: Table = Table::EMPTY;
static mut LEVEL1: Table = Table::EMPTY;

/// # Safety
/// Called once on the boot CPU, with IRQs masked, BSS zeroed and MMU off.
/// The loader and these tables must reside at their linked physical addresses.
/// The kernel mapping must come from a validated LoadPlan.
pub unsafe fn init_boot_page_tables(kernel: crate::layout::ImageMapping) {
    // SAFETY: The boot CPU exclusively owns these inactive, zeroed tables.
    // The checked kernel window fits one L1 entry and at most 16 L2 blocks.
    unsafe {
        addr_of_mut!(LEVEL1.0[0]).write(PageTableEntry::block(
            0,
            MemoryType::Device,
            BlockSize::GiB1,
        ));
        addr_of_mut!(LEVEL1.0[1]).write(PageTableEntry::block(
            0x4000_0000,
            MemoryType::Normal,
            BlockSize::GiB1,
        ));
        addr_of_mut!(ROOT.0[0]).write(PageTableEntry::table(addr_of!(LEVEL1) as usize));
        addr_of_mut!(HIGH_ROOT.0[0]).write(PageTableEntry::table(addr_of!(LEVEL1) as usize));
        let va = kernel.virtual_start();
        addr_of_mut!(HIGH_ROOT.0[(va >> 39) & 511])
            .write(PageTableEntry::table(addr_of!(KERNEL_L1) as usize));
        addr_of_mut!(KERNEL_L1.0[(va >> 30) & 511])
            .write(PageTableEntry::table(addr_of!(KERNEL_L2) as usize));
        for offset in (0..kernel.physical().size()).step_by(crate::platform::BLOCK_SIZE) {
            addr_of_mut!(KERNEL_L2.0[((va + offset) >> 21) & 511]).write(PageTableEntry::block(
                kernel.physical().start() + offset,
                MemoryType::Normal,
                BlockSize::MiB2,
            ));
        }
    }
}

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
