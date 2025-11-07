//! ARM PL011 UART driver.

use arm_pl011::Pl011Uart;
use kspin::SpinNoIrq;
use lazyinit::LazyInit;
use memory_addr::VirtAddr;

static UART: LazyInit<SpinNoIrq<Pl011Uart>> = LazyInit::new();

fn do_putchar(uart: &mut Pl011Uart, c: u8) {
    match c {
        b'\n' => {
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

/// Reads a byte from the console, or returns [`None`] if no input is available.
pub fn getchar() -> Option<u8> {
    UART.lock().getchar()
}

/// Early stage initialization of the PL011 UART driver.
pub fn init_early(uart_base: VirtAddr) {
    UART.init_once(SpinNoIrq::new(Pl011Uart::new(uart_base.as_mut_ptr())));
    UART.lock().init();
}

/// UART IRQ Handler.
pub fn irq_handler() {
    let is_receive_interrupt = UART.lock().is_receive_interrupt();
    UART.lock().ack_interrupts();
    if is_receive_interrupt {
        while let Some(c) = getchar() {
            putchar(c);
        }
    }
}
