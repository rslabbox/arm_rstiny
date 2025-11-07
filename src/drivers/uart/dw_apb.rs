//! DesignWare APB UART driver.

use dw_apb_uart::DW8250;
use kspin::SpinNoIrq;
use memory_addr::{PhysAddr, pa};

use crate::mm::phys_to_virt;
use crate::platform::{CurrentBoard, board::Board};

static UART: SpinNoIrq<DW8250> = SpinNoIrq::new(DW8250::new(
    phys_to_virt(pa!(CurrentBoard::UART_PADDR)).as_usize(),
));

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
pub fn getchar() -> Option<u8> {
    UART.lock().getchar()
}

/// UART early initialization.
pub fn init_early() {
    UART.lock().init();
}

/// UART IRQ Handler.
pub fn irq_handler(irq: usize) {
    panic!("UART IRQ Handler: {irq}");
}
