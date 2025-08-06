use core::alloc::Layout;
use linked_list_allocator::LockedHeap;

use crate::println;

pub const PAGE_SIZE: usize = 0x1000;

bitflags::bitflags! {
    pub struct MemFlags: usize {
        const READ          = 1 << 0;
        const WRITE         = 1 << 1;
        const EXECUTE       = 1 << 2;
        const USER          = 1 << 3;
        const DEVICE        = 1 << 4;
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap = LockedHeap::empty();

#[alloc_error_handler]
pub fn handle_alloc_error(layout: Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

unsafe extern "C" {
    unsafe static __heap_start: u8;
    unsafe static __heap_end: u8;
}

pub fn init_heap() {
    unsafe {
        let heap_start = &__heap_start as *const u8 as usize;
        let heap_end = &__heap_end as *const u8 as usize;
        let heap_size = heap_end - heap_start;

        println!(
            "Initializing kernel heap at: ({:#x}, {:#x}), size: {}MB",
            heap_start,
            heap_end,
            heap_size / (1024 * 1024)
        );
        HEAP_ALLOCATOR.lock().init(heap_start as *mut u8, heap_size);
    }
}
