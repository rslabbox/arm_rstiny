//! Kernel configuration constants.

pub const BOOT_STACK_SIZE: usize = 0x40000;
pub const PHYS_VIRT_OFFSET: usize = 0xffff_0000_0000_0000;
pub const HEAP_ALLOCATOR_SIZE: usize = 0x1000000; // 16MB
pub const TICKS_PER_SEC: usize = 1000; // 1000 ticks per second (1ms per tick)

// Task-related constants
pub const DEFAULT_TASK_STACK_SIZE: usize = 0x2000; // 8KB default task stack
pub const DEFAULT_TIME_SLICE_MS: u32 = 10; // 10ms time slice
pub const MAX_TASK_PRIORITY: u8 = 31; // Maximum priority value (0 is highest)
