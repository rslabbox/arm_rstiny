//! Wrapper for KernelFunc interface calls
//!
//! This module provides a simplified interface for calling kernel functions
//! through the crate_interface mechanism.

use memory_addr::{pa, va};

use crate::utils::heap_allocator::global_allocator;

/// Wrapper struct for kernel function calls
pub struct UseKernelFunc;

impl UseKernelFunc {
    /// Convert virtual address to physical address
    #[inline]
    pub fn virt_to_phys(addr: usize) -> usize {
        crate::arch::mem::virt_to_phys(va!(addr)).as_usize()
    }

    /// Convert physical address to virtual address
    #[inline]
    pub fn phys_to_virt(addr: usize) -> usize {
        crate::arch::mem::phys_to_virt(pa!(addr)).as_usize()
    }

    /// Allocate DMA coherent memory
    ///
    /// # Returns
    /// (virtual_address, physical_address)
    #[inline]
    pub fn dma_alloc_coherent(pages: usize) -> (usize, usize) {
        extern crate alloc;
        use alloc::alloc::{Layout, alloc};

        // 每页 4KB
        const PAGE_SIZE: usize = 4096;
        let size = pages * PAGE_SIZE;

        // 创建对齐到页边界的布局
        let layout = Layout::from_size_align(size, PAGE_SIZE).unwrap();

        unsafe {
            let vaddr = alloc(layout) as usize;
            if vaddr == 0 {
                panic!("DMA allocation failed");
            }
            let paddr = Self::virt_to_phys(vaddr);
            (vaddr, paddr)
        }
    }

    /// Free DMA coherent memory
    #[inline]
    pub fn dma_free_coherent(vaddr: usize, pages: usize) {
        extern crate alloc;
        use alloc::alloc::{Layout, dealloc};

        const PAGE_SIZE: usize = 4096;
        let size = pages * PAGE_SIZE;
        let layout = Layout::from_size_align(size, PAGE_SIZE).unwrap();

        unsafe {
            dealloc(vaddr as *mut u8, layout);
        }
    }

    /// Get current time in microseconds
    #[inline]
    pub fn get_time_us() -> u64 {
        let ticks = crate::arch::device::generic_timer::boot_ticks();
        let nanos = crate::arch::device::generic_timer::ticks_to_nanos(ticks);
        nanos / crate::arch::device::generic_timer::NANOS_PER_MICROS
    }

    /// Busy wait for specified duration
    #[inline]
    pub fn busy_wait(duration: core::time::Duration) {
        let start_us = Self::get_time_us();
        let duration_us = duration.as_micros() as u64;
        let end_us = start_us + duration_us;

        // 忙等待直到时间到达
        while Self::get_time_us() < end_us {
            core::hint::spin_loop();
        }
    }
}
