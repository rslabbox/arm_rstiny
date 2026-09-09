//! Boot translation descriptors, storage, and temporary mapping construction.
use core::ptr::{addr_of, addr_of_mut};

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
pub(super) struct Table([PageTableEntry; 512]);
impl Table {
    const EMPTY: Self = Self([PageTableEntry::EMPTY; 512]);
}
pub(super) static mut ROOT: Table = Table::EMPTY;
pub(super) static mut HIGH_ROOT: Table = Table::EMPTY;
static mut KERNEL_L1: Table = Table::EMPTY;
static mut KERNEL_L2: Table = Table::EMPTY;
static mut LEVEL1: Table = Table::EMPTY;

/// # Safety
/// Called once on the boot CPU, with IRQs masked, BSS zeroed and MMU off.
/// The loader and these tables must reside at their linked physical addresses.
/// The kernel mapping must come from a validated LoadPlan.
pub unsafe fn init_boot_page_tables(kernel: crate::memory::ImageMapping) {
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
