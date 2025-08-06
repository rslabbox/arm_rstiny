pub use crate::arch::config::*;

pub const PL011_UART_BASE: usize = 0x0900_0000;
pub const PHYS_MEMORY_BASE: usize = 0x4000_0000;
pub const PHYS_MEMORY_SIZE: usize = 0x800_0000;

// Memory size

pub const PHYS_MEMORY_END: usize = PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE;

pub const BOOT_KERNEL_STACK_SIZE: usize = 4096 * 4; // 16K
pub const KERNEL_HEAP_SIZE: usize = 0x40_0000; // 4M

// Scheduler

pub const TICKS_PER_SEC: u64 = 100;
