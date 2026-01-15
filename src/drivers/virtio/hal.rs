use core::alloc::Layout;
use core::ptr::NonNull;
use alloc::alloc::{alloc_zeroed, dealloc, handle_alloc_error};
use virtio_drivers::{BufferDirection, Hal, PhysAddr};
use memory_addr::{PAGE_SIZE_4K, pa, va};
use log::trace;

pub struct VirtioHalImpl;

unsafe impl Hal for VirtioHalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let layout = Layout::from_size_align(pages * PAGE_SIZE_4K, PAGE_SIZE_4K).unwrap();
        // Safe because the layout has a non-zero size.
        let vaddr = unsafe { alloc_zeroed(layout) };
        let vaddr = if let Some(vaddr) = NonNull::new(vaddr) {
            vaddr
        } else {
            handle_alloc_error(layout)
        };
        
        let paddr = crate::mm::virt_to_phys(va!(vaddr.as_ptr() as usize)).as_usize();

        trace!("alloc DMA: paddr={:#x}, pages={}", paddr, pages);
        (paddr as u64, vaddr)
    }

    unsafe fn dma_dealloc(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        trace!("dealloc DMA: paddr={:#x}, pages={}", paddr, pages);
        let layout = Layout::from_size_align(pages * PAGE_SIZE_4K, PAGE_SIZE_4K).unwrap();
        // Safe because the memory was allocated by `dma_alloc` above using the same allocator, and
        // the layout is the same as was used then.
        unsafe {
            dealloc(vaddr.as_ptr(), layout);
        }
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, _size: usize) -> NonNull<u8> {
        let vaddr = crate::mm::phys_to_virt(pa!(paddr as usize)).as_usize();
        NonNull::new(vaddr as _).unwrap()
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        let vaddr = buffer.as_ptr() as *mut u8 as usize;
        crate::mm::virt_to_phys(va!(vaddr)).as_usize().try_into().unwrap()
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Nothing to do, as the host already has access to all memory and we didn't copy the buffer
        // anywhere else.
    }
}