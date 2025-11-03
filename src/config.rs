#![allow(unused)]
/// Configuration constants for the OS kernel.

pub const BOOT_STACK_SIZE: usize = 0x40000;
pub const PHYS_VIRT_OFFSET: usize = 0xffff_0000_0000_0000;
pub const HEAP_ALLOCATOR_SIZE: usize = 0x1000000; // 16MB

pub mod devices {
    /// UART physical address
    pub const UART_PADDR: usize = 0xfeb5_0000;
    pub const GICD_BASE: usize = 0xfe60_0000;
    pub const GICR_BASE: usize = 0xfe68_0000;
}
