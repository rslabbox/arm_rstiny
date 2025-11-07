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
