//! Fixed direct-map addresses and runtime translation of the kernel image.
include!(concat!(env!("OUT_DIR"), "/platform.rs"));
pub const KERNEL_OFFSET: usize = 0xffff_0000_0000_0000;
pub const PL011_UART_BASE: usize = KERNEL_OFFSET + UART_BASE;
pub const fn phys_to_virt(address: usize) -> usize {
    KERNEL_OFFSET + address
}
/// Kernel image addresses use a runtime mapping; the direct map stays fixed.
pub fn virt_to_phys(address: usize) -> usize {
    unsafe extern "C" {
        static skernel: u8;
        static ekernel: u8;
    }
    let start = core::ptr::addr_of!(skernel) as usize;
    let end = core::ptr::addr_of!(ekernel) as usize;
    if (start..=end).contains(&address) {
        crate::arch::boot::information().kernel_physical + address - start
    } else {
        address - KERNEL_OFFSET
    }
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
