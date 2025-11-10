//! Wrapper for KernelFunc interface calls
//!
//! This module provides a simplified interface for calling kernel functions
//! through the crate_interface mechanism.

use core::alloc::Layout;

use memory_addr::{pa, va};

use crate::mm::Aligned4K;
const PAGE_SIZE: usize = 4096;
/// Wrapper struct for kernel function calls
pub struct UseKernelFunc;

impl UseKernelFunc {
    /// Convert virtual address to physical address
    #[inline]
    pub fn virt_to_phys(addr: usize) -> usize {
        crate::mm::virt_to_phys(va!(addr)).as_usize()
    }

    /// Convert physical address to virtual address
    #[inline]
    pub fn phys_to_virt(addr: usize) -> usize {
        crate::mm::phys_to_virt(pa!(addr)).as_usize()
    }

    /// Allocate DMA coherent memory
    ///
    /// # Returns
    /// (virtual_address, physical_address)
    #[inline]
    pub fn dma_alloc_coherent(pages: usize) -> (usize, usize) {
        let size = pages * PAGE_SIZE;
        let layout = Layout::from_size_align(size, PAGE_SIZE).expect("Invalid layout");
        unsafe {
            let allocator = crate::mm::allocator::global_allocator();
            let ptr = allocator.alloc(layout);

            if ptr.is_null() {
                panic!("Failed to allocate DMA memory");
            }

            let virt_addr = ptr as usize;
            let phys_addr = crate::mm::virt_to_phys(va!(virt_addr)).as_usize();

            core::ptr::write_bytes(ptr, 0, size);

            (virt_addr, phys_addr)
        }
    }

    /// Free DMA coherent memory
    #[inline]
    pub fn dma_free_coherent(vaddr: usize, pages: usize) {
        let size = pages * PAGE_SIZE;
        let layout = Layout::from_size_align(size, PAGE_SIZE).expect("Invalid layout");
        unsafe {
            let allocator = crate::mm::allocator::global_allocator();
            allocator.dealloc(vaddr as *mut u8, layout);
        }
    }

    /// Get current time in microseconds
    #[inline]
    pub fn get_time_us() -> u64 {
        let boot_ticks = crate::drivers::timer::boot_ticks();
        let nanos = crate::drivers::timer::ticks_to_nanos(boot_ticks);
        nanos / 1000
    }

    /// Busy wait for specified duration
    #[inline]
    pub fn busy_wait(duration: core::time::Duration) {
        crate::drivers::timer::busy_wait(duration);
    }
}
