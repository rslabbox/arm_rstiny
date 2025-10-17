use core::alloc::Layout;

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


use talc::*;

use crate::config::HEAP_ALLOCATOR_SIZE;

static mut ARENA: [u8; HEAP_ALLOCATOR_SIZE] = [0; HEAP_ALLOCATOR_SIZE];

#[global_allocator]
static ALLOCATOR: Talck<spin::Mutex<()>, ClaimOnOom> = Talc::new(unsafe {
    // if we're in a hosted environment, the Rust runtime may allocate before
    // main() is called, so we need to initialize the arena automatically
    ClaimOnOom::new(Span::from_array(core::ptr::addr_of!(ARENA).cast_mut()))
})
.lock();

#[alloc_error_handler]
pub fn handle_alloc_error(layout: Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}
