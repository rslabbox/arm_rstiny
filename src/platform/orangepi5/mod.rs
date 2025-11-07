//! OrangePi 5 Plus board support.

pub mod config;

use super::board::Board;

pub struct OrangePi5Plus;

impl Board for OrangePi5Plus {
    const NAME: &'static str = "OrangePi 5 Plus";
    const UART_PADDR: usize = config::UART_PADDR;
    const GICD_BASE: usize = config::GICD_BASE;
    const GICR_BASE: usize = config::GICR_BASE;

    fn init() {
        // Board-specific initialization
    }
}
