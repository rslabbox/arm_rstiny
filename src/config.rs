pub use crate::arch::config::*;

pub const PL011_UART_BASE: usize = 0x0900_0000;
pub const KERNEL_BASE_PADDR: usize = 0x4008_0000;
pub const KERNEL_BASE_VADDR: usize = 0xffff_0000_4008_0000;
pub const PHYS_MEMORY_BASE: usize = 0x4000_0000;
pub const PHYS_MEMORY_SIZE: usize = 0x800_0000;
#[rustfmt::skip]
pub const MMIO_REGIONS: &[(usize, usize)] = &[
    (0x0900_0000, 0x1000),
    (0x0800_0000, 0x2_0000),
];

// Memory size

pub const PHYS_MEMORY_END: usize = PHYS_MEMORY_BASE + PHYS_MEMORY_SIZE;

pub const BOOT_KERNEL_STACK_SIZE: usize = 4096 * 4; // 16K
pub const USER_STACK_SIZE: usize = 4096 * 4; // 16K
pub const USER_STACK_BASE: usize = USER_ASPACE_BASE + USER_ASPACE_SIZE - USER_STACK_SIZE;
pub const KERNEL_STACK_SIZE: usize = 4096 * 4; // 16K
pub const KERNEL_HEAP_SIZE: usize = 0x40_0000; // 4M

// SMP

pub const MAX_CPUS: usize = 1;

// Scheduler

pub const TICKS_PER_SEC: u64 = 100;
