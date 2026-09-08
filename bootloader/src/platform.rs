//! Fixed QEMU virt address spaces; image placement is selected from free RAM.
pub const UART_BASE: usize = 0x0900_0000;
pub const RAM_START: usize = 0x4000_0000;
pub const RAM_END: usize = 0x4800_0000;
/// Leave QEMU's reset stub and firmware data below the allocation window.
pub const FIRMWARE_END: usize = RAM_START + 2 * 1024 * 1024;
pub const KERNEL_VA_START: usize = 0xffff_8000_0000_0000;
pub const KERNEL_VA_END: usize = KERNEL_VA_START + 32 * 1024 * 1024;
pub const BLOCK_SIZE: usize = 2 * 1024 * 1024;
