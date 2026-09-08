use linked_list_allocator::LockedHeap;

#[global_allocator]
static HEAP_ALLOCATOR: LockedHeap = LockedHeap::empty();

unsafe extern "C" {
    unsafe static __heap_start: u8;
    unsafe static __heap_end: u8;
}

pub fn init_heap() {
    unsafe {
        let heap_start = &__heap_start as *const u8 as usize;
        let heap_end = &__heap_end as *const u8 as usize;
        let heap_size = heap_end - heap_start;

        assert!(heap_start >= crate::config::RAM_START);
        assert!(heap_end <= crate::config::RAM_END);
        HEAP_ALLOCATOR.lock().init(heap_start as *mut u8, heap_size);
    }
}
