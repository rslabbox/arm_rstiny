//! Platform module - Board-specific configuration and support.
//!
//! This module provides abstractions for different hardware platforms,
//! allowing the kernel to run on multiple boards with different configurations.

pub mod board;
pub mod orangepi5;
pub mod qemu_virt;

// Select the current board based on compile-time features
#[cfg(feature = "opi5p")]
pub type CurrentBoard = orangepi5::OrangePi5Plus;

#[cfg(feature = "qemu")]
pub type CurrentBoard = qemu_virt::QemuVirt;

// Default to OrangePi 5 Plus if no feature is specified
#[cfg(not(any(feature = "opi5p", feature = "qemu")))]
pub type CurrentBoard = orangepi5::OrangePi5Plus;
