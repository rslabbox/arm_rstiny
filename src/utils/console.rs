use arm_pl011::Pl011Uart;
use kspin::SpinNoIrq;

use crate::config::PL011_UART_BASE;

static UART: SpinNoIrq<Pl011Uart> = SpinNoIrq::new(Pl011Uart::new(PL011_UART_BASE as *mut u8));

/// Writes a byte to the console.
pub fn console_putchar(c: usize) {
    let mut uart = UART.lock();
    match c as u8 {
        b'\n' => {
            uart.putchar(b'\r');
            uart.putchar(b'\n');
        }
        c => uart.putchar(c),
    }
}
