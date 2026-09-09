//! Fixed platform constants and kernel policy. Address conversion lives in memory/address.rs.
include!(concat!(env!("OUT_DIR"), "/platform.rs"));
pub const KERNEL_OFFSET: usize = 0xffff_0000_0000_0000;
pub const PL011_UART_BASE: usize = KERNEL_OFFSET + UART_BASE;
pub const PA_MAX_BITS: usize = 40;
pub const PAGE_SIZE: usize = 4096;

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
