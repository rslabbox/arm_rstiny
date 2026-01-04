//! Address translation utilities.

use memory_addr::{PhysAddr, VirtAddr, pa, va};

use crate::config::kernel::{TINYENV_KIMAGE_VADDR, kernel_phys_base};

/// The high bits for TTBR1 virtual addresses (kernel space).
/// All kernel virtual addresses start with 0xffff_0000_...
const KERNEL_VA_HIGH_BITS: usize = 0xffff_0000_0000_0000;

/// Convert physical address to virtual address.
///
/// For kernel addresses (PA >= kernel_phys_base), uses KIMAGE_VADDR mapping.
/// For device/lower addresses, uses direct offset mapping (PA + 0xffff_0000_0000_0000).
#[inline]
pub fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
    let pa = paddr.as_usize();
    let kbase = kernel_phys_base();
    
    if kbase > 0 && pa >= kbase {
        // Kernel address: VA = PA - kernel_phys_base + KIMAGE_VADDR
        va!(pa.wrapping_sub(kbase).wrapping_add(TINYENV_KIMAGE_VADDR))
    } else {
        // Device/low address: direct mapping VA = PA + KERNEL_VA_HIGH_BITS
        va!(pa.wrapping_add(KERNEL_VA_HIGH_BITS))
    }
}

/// Convert virtual address to physical address.
///
/// For kernel addresses (VA >= KIMAGE_VADDR), uses KIMAGE_VADDR mapping.
/// For device/lower addresses, uses direct offset mapping.
#[inline]
pub fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
    let va = vaddr.as_usize();
    
    if va >= TINYENV_KIMAGE_VADDR {
        // Kernel address: PA = VA - KIMAGE_VADDR + kernel_phys_base
        pa!(va.wrapping_sub(TINYENV_KIMAGE_VADDR).wrapping_add(kernel_phys_base()))
    } else {
        // Device/low address: PA = VA - KERNEL_VA_HIGH_BITS
        pa!(va.wrapping_sub(KERNEL_VA_HIGH_BITS))
    }
}
