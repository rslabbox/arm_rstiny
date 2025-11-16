//! DesignWare APB UART driver.

use dw_apb_uart::DW8250;
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use crate::TinyResult;
use crate::error::TinyError;
use memory_addr::VirtAddr;

static UART: LazyInit<SpinNoIrq<DW8250>> = LazyInit::new();

/// Writes a byte to the console.
pub fn putchar(c: u8) {
    let mut uart = UART.lock();
    match c {
        b'\r' | b'\n' => {
            uart.putchar(b'\r');
            uart.putchar(b'\n');
        }
        c => uart.putchar(c),
    }
}

/// Reads a byte from the console, or returns [`None`] if no input is available.
#[allow(unused)]
pub fn getchar() -> Option<u8> {
    UART.lock().getchar()
}

/// UART early initialization.
#[allow(unused)]
pub fn init_early(uart_base: VirtAddr) {
    UART.init_once(SpinNoIrq::new(DW8250::new(uart_base.as_usize())));
    // UART.lock().init();
}

/// UART IRQ Handler.
#[allow(unused)]
pub fn irq_handler(irq: usize) -> TinyResult<()> {
    error!("UART IRQ Handler invoked unexpectedly: {irq}");
    Err(TinyError::UartIrqUnexpected(irq))
}
