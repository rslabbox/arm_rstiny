//! QEMU virt board configuration.

use arm_gic::IntId;

pub const BOARD_NAME: &str = "QEMU virt";

/// QEMU virt board constants.
pub const UART_PADDR: usize = 0x0900_0000;
pub const GICD_BASE: usize = 0x0800_0000;
pub const GICR_BASE: usize = 0x080a_0000;

pub const UART_IRQ: IntId = IntId::spi(1);