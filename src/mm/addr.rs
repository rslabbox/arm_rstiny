//! Address translation utilities.

use memory_addr::{PhysAddr, VirtAddr, pa, va};

use crate::config::kernel::PHYS_VIRT_OFFSET;

/// Convert physical address to virtual address.
pub const fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    va!(paddr.as_usize() + PHYS_VIRT_OFFSET)
}

/// Convert virtual address to physical address.
pub const fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
    pa!(vaddr.as_usize() - PHYS_VIRT_OFFSET)
}
