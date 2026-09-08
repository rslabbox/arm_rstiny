//! Build-time QEMU virt platform description; no runtime DTB parsing.
include!(concat!(env!("OUT_DIR"), "/platform.rs"));
pub const KERNEL_OFFSET: usize = 0xffff_0000_0000_0000;
pub const PL011_UART_BASE: usize = KERNEL_OFFSET + UART_BASE;
pub const fn phys_to_virt(address: usize) -> usize {
    KERNEL_OFFSET + address
}
pub const fn virt_to_phys(address: usize) -> usize {
    address - KERNEL_OFFSET
}
pub const PA_MAX_BITS: usize = 40;
pub const PAGE_SIZE: usize = 4096;

bitflags::bitflags! {
    pub struct MemFlags: usize {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
        const USER = 1 << 3;
        const DEVICE = 1 << 4;
    }
}
