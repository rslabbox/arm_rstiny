//! Log and panic console backed by the PL011 polling driver.
mod pl011;

use crate::config::PL011_UART_BASE;
use core::fmt::{self, Write};
use pl011::Pl011Uart;

fn uart() -> Pl011Uart {
    // SAFETY: boot maps this aligned PL011 page as Device memory. Ordinary
    // output is serialized by the logger; panic takes over with IRQs masked
    // and never returns. The current platform is single-core.
    unsafe { Pl011Uart::new(PL011_UART_BASE as *mut u8) }
}

pub fn init() -> bool {
    uart().init()
}

pub fn put_byte(byte: u8) -> fmt::Result {
    uart().putchar(byte)
}

pub(super) struct Writer;
impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                put_byte(b'\r')?;
            }
            put_byte(byte)?;
        }
        Ok(())
    }
}

/// Emergency output does not use the logger, its lock, or its level filter.
pub fn panic_print(info: &core::panic::PanicInfo<'_>) {
    if init() {
        let _ = Writer.write_fmt(format_args!("kernel panic: {}\n", info));
    }
}
