use arm_pl011::Pl011Uart;
use core::fmt;
use core::fmt::Write;
use kspin::SpinNoIrq;

static PL011_UART_BASE: usize = 0x0900_0000; // Base address for PL011 UART, as per the configuration.
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

pub struct SimpleLogger;

impl Write for SimpleLogger {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            console_putchar(c as usize);
        }
        Ok(())
    }
}

// 实现 print! 和 println! 宏
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub fn _print(args: fmt::Arguments) {
    SimpleLogger.write_fmt(args).unwrap();
}
