//! OrangePi 5 Plus board configuration.

pub const BOARD_NAME: &str = "OrangePi 5 Plus";

/// OrangePi 5 Plus board constants.
pub const UART_PADDR: usize = 0xfeb5_0000;
pub const GICD_BASE: usize = 0xfe60_0000;
pub const GICR_BASE: usize = 0xfe68_0000;
