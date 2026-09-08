//! Fixed QEMU virt contract. No DTB discovery or free-RAM export yet.
pub const KERNEL_OFFSET: usize = 0xffff_0000_0000_0000;
pub const PL011_UART_BASE: usize = KERNEL_OFFSET + 0x0900_0000;
pub const GICD_BASE: usize = 0x0800_0000;
pub const GICD_SIZE: usize = 0x1_0000;
pub const GICR_BASE: usize = 0x080a_0000;
pub const GICR_SIZE: usize = 0x2_0000; // One GICv3 redistributor, including SGI/PPI frame.
pub const fn phys_to_virt(address: usize) -> usize {
    KERNEL_OFFSET + address
}
pub const fn virt_to_phys(address: usize) -> usize {
    address - KERNEL_OFFSET
}
pub const RAM_START: usize = 0x4000_0000;
pub const RAM_END: usize = 0x4800_0000;
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
