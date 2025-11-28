//! Kernel configuration constants.

pub const BOOT_STACK_SIZE: usize = 0x40000;
pub const PHYS_VIRT_OFFSET: usize = 0xffff_0000_0000_0000;
pub const HEAP_ALLOCATOR_SIZE: usize = 0x1000000; // 16MB
pub const TICKS_PER_SEC: usize = 1000; // 1000 ticks per second (1ms per tick)

// Multi-core configuration
pub const MAX_CPUS: usize = 8;
pub const SECONDARY_STACK_SIZE: usize = 0x10000; // 64KB per secondary CPU

// Task scheduling configuration
pub const TASK_STACK_SIZE: usize = 0x10000; // 64KB per task
