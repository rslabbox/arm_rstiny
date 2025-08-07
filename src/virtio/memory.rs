use core::{alloc::Layout, ptr::NonNull};

pub trait VirtioAlloc {
    fn allocate(layout: Layout) -> NonNull<u8>;
    fn deallocate(ptr: NonNull<u8>, layout: Layout);
}
