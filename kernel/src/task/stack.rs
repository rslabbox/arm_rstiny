//! Owned, page-aligned kernel stack with an unmapped lower guard page.
use crate::memory::{Error, PAGE_SIZE};
use alloc::alloc::{alloc_zeroed, dealloc};
use core::{alloc::Layout, ptr::NonNull};

const STACK_SIZE: usize = 64 * 1024;
pub(super) struct KernelStack {
    allocation: NonNull<u8>,
}
// SAFETY: unique allocation; moving the owner never moves the live stack.
unsafe impl Send for KernelStack {}
impl KernelStack {
    fn layout() -> Layout {
        Layout::from_size_align(STACK_SIZE + PAGE_SIZE, PAGE_SIZE).unwrap()
    }
    pub fn new() -> Result<Self, Error> {
        // SAFETY: a valid nonzero layout; null is a recoverable allocation failure.
        let allocation =
            NonNull::new(unsafe { alloc_zeroed(Self::layout()) }).ok_or(Error::NoMemory)?;
        // SAFETY: the first page belongs exclusively to this allocation and is
        // not used for stack data or allocator metadata until restored in Drop.
        unsafe { crate::arch::boot::set_heap_guard(allocation.as_ptr() as usize, true) };
        Ok(Self { allocation })
    }
    pub fn top(&self) -> usize {
        self.allocation.as_ptr() as usize + PAGE_SIZE + STACK_SIZE
    }
}
impl Drop for KernelStack {
    fn drop(&mut self) {
        // SAFETY: the scheduler has switched away permanently. Restore both
        // aliases before the allocator can write its free-list into this page.
        unsafe {
            crate::arch::boot::set_heap_guard(self.allocation.as_ptr() as usize, false);
            dealloc(self.allocation.as_ptr(), Self::layout());
        }
    }
}
