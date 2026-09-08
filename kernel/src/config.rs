//! Fixed QEMU virt contract. No DTB discovery or free-RAM export yet.
pub const PL011_UART_BASE: usize = 0x0900_0000;
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
