//! Memory management module.
//!
//! This module provides memory management facilities including:
//! - Heap allocation
//! - Physical memory management
//! - Address translation
//! - Future: Page table management, virtual memory

pub mod addr;
pub mod allocator;
pub mod phys;

#[allow(unused)]
pub use addr::{phys_to_virt, virt_to_phys};

#[allow(unused)]
pub use phys::{Aligned4K, MemRegionFlags};
