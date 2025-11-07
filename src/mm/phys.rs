//! Physical memory management.

use core::fmt;
use core::ops::{Deref, DerefMut};

bitflags::bitflags! {
    /// The flags of a physical memory region.
    #[derive(Clone, Copy)]
    pub struct MemRegionFlags: usize {
        /// Readable.
        const READ          = 1 << 0;
        /// Writable.
        const WRITE         = 1 << 1;
        /// Executable.
        const EXECUTE       = 1 << 2;
        /// Device memory. (e.g., MMIO regions)
        const DEVICE        = 1 << 4;
        /// Uncachable memory. (e.g., framebuffer)
        const UNCACHED      = 1 << 5;
        /// Reserved memory, do not use for allocation.
        const RESERVED      = 1 << 6;
        /// Free memory for allocation.
        const FREE          = 1 << 7;
    }
}

impl fmt::Debug for MemRegionFlags {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// A wrapper type for aligning a value to 4K bytes.
#[repr(align(4096))]
pub struct Aligned4K<T: Sized>(T);

impl<T: Sized> Aligned4K<T> {
    /// Creates a new [`Aligned4K`] instance with the given value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for Aligned4K<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Aligned4K<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
