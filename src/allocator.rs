use core::alloc::Layout;
use linked_list_allocator::LockedHeap;
use log::info;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

unsafe extern "C" {
    unsafe static __heap_start: u8;
    unsafe static __heap_end: u8;
}

pub fn init_heap() {
    unsafe {
        let heap_start = &__heap_start as *const u8 as usize;
        let heap_end = &__heap_end as *const u8 as usize;
        let heap_size = heap_end - heap_start;

        info!(
            "Heap: 0x{:x} - 0x{:x} (size: {} MB)",
            heap_start,
            heap_end,
            heap_size / (1024 * 1024)
        );

        ALLOCATOR.lock().init(heap_start as *mut u8, heap_size);
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}
