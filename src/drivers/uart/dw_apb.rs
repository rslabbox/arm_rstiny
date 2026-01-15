//! DesignWare APB UART driver.

use crate::TinyResult;
use dw_apb_uart::DW8250;
use crate::hal::Mutex;
use lazyinit::LazyInit;
use memory_addr::VirtAddr;

static UART: LazyInit<Mutex<DW8250>> = LazyInit::new();

fn do_putchar(uart: &mut DW8250, c: u8) {
    match c {
        b'\r' | b'\n' => {
            uart.putchar(b'\r');
            uart.putchar(b'\n');
        }
        c => uart.putchar(c),
    }
}

/// Writes a byte to the console.
pub fn putchar(c: u8) {
    do_putchar(&mut UART.lock(), c);
}

/// Writes a string to the console atomically (holding the lock for the entire string).
///
/// This prevents output from multiple CPUs from being interleaved.
pub fn puts(s: &str) {
    let mut uart = UART.lock();
    for c in s.bytes() {
        do_putchar(&mut uart, c);
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
    UART.init_once(Mutex::new(DW8250::new(uart_base.as_usize())));
    UART.lock().init();
}

/// UART IRQ Handler.
#[allow(unused)]
pub fn irq_handler(irq: usize) -> TinyResult<()> {
    error!("UART IRQ Handler invoked unexpectedly: {irq}");
    anyhow::bail!("UART IRQ handler invoked unexpectedly: {}", irq)
}
