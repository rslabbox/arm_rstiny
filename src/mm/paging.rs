use core::{fmt::Debug};

// use crate::mm::memory_set::MapArea;

use super::{MemFlags, PhysAddr};

pub trait GenericPTE: Debug + Clone + Copy + Sync + Send + Sized {
    // Create a page table entry point to a terminate page or block.
    fn new_page(paddr: PhysAddr, flags: MemFlags, is_block: bool) -> Self;
    // Create a page table entry point to a next level page table.
    fn new_table(paddr: PhysAddr) -> Self;

    /// Returns the physical address mapped by this entry.
    fn paddr(&self) -> PhysAddr;
    /// Returns the flags of this entry.
    fn flags(&self) -> MemFlags;
}
