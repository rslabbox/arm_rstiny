pub const PL011_UART_BASE: usize = 0x0900_0000;

pub const BOOT_KERNEL_STACK_SIZE: usize = 4096 * 4; // 16K
pub const KERNEL_HEAP_SIZE: usize = 0x40_0000; // 4M
pub const USER_ASPACE_BASE: usize = 0;
pub const USER_ASPACE_SIZE: usize = 0xffff_ffff_f000;

pub const PHYS_VIRT_OFFSET: usize = 0x0000_0000_0000;

pub const PA_MAX_BITS: usize = 40; // 1TB
pub const VA_MAX_BITS: usize = 48;
