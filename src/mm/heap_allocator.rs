use buddy_system_allocator::Heap;
use kspin::SpinNoIrq;
use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;
use core::ptr::NonNull;

use crate::config::KERNEL_HEAP_SIZE;

struct LockedHeap(SpinNoIrq<Heap<32>>);

impl LockedHeap {
    pub const fn empty() -> Self {
        LockedHeap(SpinNoIrq::new(Heap::<32>::new()))
    }

    pub fn init(&self, start: usize, size: usize) {
        unsafe { self.0.lock().init(start, size) };
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0
            .lock()
            .alloc(layout)
            .ok()
            .map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.0.lock().dealloc(unsafe { NonNull::new_unchecked(ptr) }, layout)
    }
}
#[cfg_attr(not(test), global_allocator)]
static HEAP_ALLOCATOR: LockedHeap = LockedHeap::empty();

#[cfg_attr(not(test), alloc_error_handler)]
pub fn handle_alloc_error(layout: Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

static mut HEAP_SPACE: [u64; KERNEL_HEAP_SIZE / size_of::<u64>()] =
    [0; KERNEL_HEAP_SIZE / size_of::<u64>()];

pub fn init_heap() {
    #[allow(static_mut_refs)]
    let heap_start = unsafe { HEAP_SPACE.as_ptr() as usize };
    println!(
        "Initializing kernel heap at: [{:#x}, {:#x})",
        heap_start,
        heap_start + KERNEL_HEAP_SIZE
    );
    HEAP_ALLOCATOR.init(heap_start, KERNEL_HEAP_SIZE);
}