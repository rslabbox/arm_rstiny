//! AArch64 page-table descriptors, traversal and construction.
mod page_table;
pub(crate) mod paging;
pub(crate) use page_table::PageTableEntry;
