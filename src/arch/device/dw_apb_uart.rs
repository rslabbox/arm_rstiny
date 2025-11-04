//! snps,dw-apb-uart serial driver

use dw_apb_uart::DW8250;
use kspin::SpinNoIrq;
use memory_addr::{PhysAddr, pa};

use crate::arch::mem::phys_to_virt;

const UART_BASE: PhysAddr = pa!(crate::config::devices::UART_PADDR);

static UART: SpinNoIrq<DW8250> = SpinNoIrq::new(DW8250::new(phys_to_virt(UART_BASE).as_usize()));

/// Writes a byte to the console.
#[allow(dead_code)]
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
#[allow(dead_code)]
fn getchar() -> Option<u8> {
    UART.lock().getchar()
}

/// UART simply initialize
#[allow(dead_code)]
pub fn init_early() {
    UART.lock().init();
}

/// UART IRQ Handler
#[allow(dead_code)]
pub fn irq_handle(irq: usize) {
    panic!("Uart IRQ Handler: {irq}");
}
