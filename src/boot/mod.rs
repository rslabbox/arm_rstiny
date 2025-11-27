//! Boot module - Early kernel initialization.
//!
//! This module contains all the code needed to boot the kernel, including:
//! - Assembly entry point with Linux image header
//! - Exception level switching (EL3/EL2 -> EL1)
//! - MMU initialization
//! - Boot page table setup

pub mod entry;
pub mod init;
pub mod mmu;

use crate::config::kernel::PHYS_VIRT_OFFSET;

/// Returns the physical address of the secondary CPU entry point.
///
/// This is used by the primary CPU to start secondary CPUs via PSCI cpu_on.
pub fn secondary_entry_paddr() -> usize {
    // The entry point virtual address minus the offset gives the physical address
    entry::_start_secondary as usize - PHYS_VIRT_OFFSET
}
