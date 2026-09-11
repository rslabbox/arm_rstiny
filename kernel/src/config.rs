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

/// Platform IRQ table (docs/irq.md §3.1), generated into `IRQ_LINES` from the
/// DTB: `(INTID, level, kind)` per user-authorizable line. VirtIO MMIO slot
/// lines come first in ascending slot order (kind `IRQ_KIND_VIRTIO_SLOT`);
/// other device lines (the PL011 today) follow (kind `IRQ_KIND_DEVICE`). The
/// timer PPI and every unmapped line stay kernel-owned: `IRQControl_Get`
/// rejects them, so a user driver can never touch kernel or foreign sources.
/// Authorization policy is this table, not arithmetic over window constants.
pub fn user_irq_level(intid: u32) -> Option<bool> {
    IRQ_LINES
        .iter()
        .find(|line| line.0 == intid as u64)
        .map(|&(_, level, _)| level != 0)
}

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
