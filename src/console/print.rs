//! Console print macros.
//!
//! This module provides thread-safe printing that prevents output from
//! multiple CPUs from being interleaved.

use core::fmt::{self, Write};
use crate::hal::Mutex;

static PRINT_LOCK: Mutex<()> = Mutex::new(());

/// Buffer size for formatting output before sending to UART.
/// This should be large enough for most log lines.
const PRINT_BUFFER_SIZE: usize = 512;

/// A printer that formats into a fixed-size buffer, then outputs atomically.
struct BufferedPrinter {
    buffer: [u8; PRINT_BUFFER_SIZE],
    pos: usize,
}

impl BufferedPrinter {
    const fn new() -> Self {
        Self {
            buffer: [0; PRINT_BUFFER_SIZE],
            pos: 0,
        }
    }

    fn flush(&mut self) {
        if self.pos > 0 {
            // Safety: buffer contains valid UTF-8 since we only write from str
            let s = unsafe { core::str::from_utf8_unchecked(&self.buffer[..self.pos]) };
            // Output the entire string atomically (UART lock held for whole string)
            crate::drivers::uart::puts(s);
            self.pos = 0;
        }
    }
}

impl Write for BufferedPrinter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &byte in s.as_bytes() {
            if self.pos >= PRINT_BUFFER_SIZE {
                // Buffer full, flush it
                self.flush();
            }
            self.buffer[self.pos] = byte;
            self.pos += 1;
        }
        Ok(())
    }
}

impl Drop for BufferedPrinter {
    fn drop(&mut self) {
        self.flush();
    }
}

pub fn _print(args: fmt::Arguments) {
    // Acquire global print lock - only one CPU can print at a time
    let _guard = PRINT_LOCK.lock();

    let mut printer = BufferedPrinter::new();
    // Ignore write errors - printing should not panic
    let _ = printer.write_fmt(args);
    // Flush happens automatically in Drop
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
