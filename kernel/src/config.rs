//! Fixed platform constants and kernel policy. Address conversion lives in memory/address.rs.
include!(concat!(env!("OUT_DIR"), "/platform.rs"));
pub const KERNEL_OFFSET: usize = 0xffff_0000_0000_0000;
pub const PL011_UART_BASE: usize = KERNEL_OFFSET + UART_BASE;
pub const PA_MAX_BITS: usize = 40;
pub const PAGE_SIZE: usize = 4096;

/// Physical regions the boot partitioner never hands to users.
/// QEMU firmware/reset data below the allocation window.
pub const FIRMWARE_END: usize = RAM_START + 2 * 1024 * 1024;
/// Rust bootloader image; fixed by its linker script. Reserved conservatively
/// because the kernel is not told the loader's exact end address.
pub const LOADER_START: usize = 0x4400_0000;
pub const LOADER_END: usize = 0x4420_0000;

/// Scheduling tick policy, converted to counter ticks by task::tick.
pub const TICK_NS: u64 = 10_000_000;

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct MemFlags: usize {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
        const USER = 1 << 3;
        const DEVICE = 1 << 4;
    }
}
