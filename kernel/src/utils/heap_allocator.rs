use crate::utils::single_core::SingleCore;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use linked_list_allocator::Heap;

struct KernelHeap(SingleCore<Heap>);

// SAFETY: heap access is exclusive within the single-core, IRQ-masked kernel.
unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0
            .borrow_mut()
            .allocate_first_fit(layout)
            .map_or(core::ptr::null_mut(), NonNull::as_ptr)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: GlobalAlloc caller supplies a live allocation and its layout.
        unsafe {
            self.0
                .borrow_mut()
                .deallocate(NonNull::new_unchecked(ptr), layout);
        }
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: KernelHeap = KernelHeap(SingleCore::new(Heap::empty()));

unsafe extern "C" {
    unsafe static __heap_start: u8;
    unsafe static __heap_end: u8;
}

pub fn init_heap() {
    unsafe {
        let heap_start = &__heap_start as *const u8 as usize;
        let heap_end = &__heap_end as *const u8 as usize;
        let heap_size = heap_end - heap_start;

        assert!(crate::config::virt_to_phys(heap_start) >= crate::config::RAM_START);
        assert!(crate::config::virt_to_phys(heap_end) <= crate::config::RAM_END);
        HEAP_ALLOCATOR
            .0
            .borrow_mut()
            .init(heap_start as *mut u8, heap_size);
    }
}
