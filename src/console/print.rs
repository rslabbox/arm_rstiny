//! Console print macros.

use core::fmt;

pub struct ConsolePrinter;

impl fmt::Write for ConsolePrinter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            crate::drivers::uart::putchar(c as u8);
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments) {
    use fmt::Write;
    ConsolePrinter.write_fmt(args).unwrap();
}

/// Simple console print operation.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        $crate::console::print::_print(format_args!($($arg)*))
    });
}

/// Simple console print operation with newline.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
