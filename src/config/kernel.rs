//! Kernel configuration constants.

pub const BOOT_STACK_SIZE: usize = 0x40000;
pub const PHYS_VIRT_OFFSET: usize = 0xffff_0000_0000_0000;
pub const HEAP_ALLOCATOR_SIZE: usize = 0x1000000; // 16MB
pub const TICKS_PER_SEC: usize = 100; // 100 ticks per second
