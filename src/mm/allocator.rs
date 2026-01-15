//! Heap allocator implementation.

use core::alloc::{GlobalAlloc, Layout};

use talc::*;

use crate::{config::kernel::HEAP_ALLOCATOR_SIZE, hal::SpinNoIrq};

static mut ARENA: [u8; HEAP_ALLOCATOR_SIZE] = [0; HEAP_ALLOCATOR_SIZE];

#[global_allocator]
static ALLOCATOR: Talck<SpinNoIrq, ClaimOnOom> = Talc::new(unsafe {
    // if we're in a hosted environment, the Rust runtime may allocate before
    // main() is called, so we need to initialize the arena automatically
    ClaimOnOom::new(Span::from_array(core::ptr::addr_of!(ARENA).cast_mut()))
})
.lock();

#[alloc_error_handler]
pub fn handle_alloc_error(layout: Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

#[allow(dead_code)]
pub fn global_allocator() -> &'static dyn GlobalAlloc {
    &ALLOCATOR
}
