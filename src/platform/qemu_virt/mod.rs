//! QEMU virt board support.

pub mod config;

use super::board::Board;

pub struct QemuVirt;

impl Board for QemuVirt {
    const NAME: &'static str = "QEMU virt";
    const UART_PADDR: usize = config::UART_PADDR;
    const GICD_BASE: usize = config::GICD_BASE;
    const GICR_BASE: usize = config::GICR_BASE;

    fn init() {
        // Board-specific initialization
    }
}
