//! Platform module - Board-specific configuration and support.
//!
//! This module provides abstractions for different hardware platforms,
//! allowing the kernel to run on multiple boards with different configurations.

#[cfg(feature = "opi5p")]
pub mod orangepi5;

#[cfg(feature = "opi5p")]
pub use orangepi5::*;

#[cfg(feature = "qemu")]
pub mod qemu_virt;

#[cfg(feature = "qemu")]
pub use qemu_virt::*;