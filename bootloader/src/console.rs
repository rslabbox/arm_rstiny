//! Polled boot diagnostics and terminal failure handling.
use crate::{pl011, platform};
use core::{
    arch::asm,
    fmt::{self, Write},
};

/// Print a boot message followed by a newline, regardless of log filtering.
/// UART must already be initialized; output is best-effort and allocation-free.
macro_rules! bootinfo {
    () => {
        $crate::console::print_bootinfo(core::format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::console::print_bootinfo(core::format_args!($($arg)*))
    };
}
pub(crate) use bootinfo;

pub(crate) fn print_bootinfo(arguments: fmt::Arguments<'_>) {
    // A UART timeout must not turn an informational message into a boot failure.
    let _ = writeln!(Console, "{arguments}");
}

pub(crate) fn init() {
    uart().init();
}

fn uart() -> pl011::Pl011Uart {
    // SAFETY: one CPU, IRQs masked; this physical UART address is accessible
    // before the MMU and remains mapped Device through the temporary tree.
    unsafe { pl011::Pl011Uart::new(platform::UART_BASE as *mut u8) }
}
struct Console;
impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' {
                put_byte(b'\r')?;
            }
            put_byte(byte)?;
        }
        Ok(())
    }
}
fn put_byte(byte: u8) -> fmt::Result {
    uart().putchar(byte)
}
pub(crate) fn fail(message: impl fmt::Display) -> ! {
    let _ = writeln!(Console, "bootloader: error: {message}");
    bootloader_halt()
}
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn bootloader_halt() -> ! {
    loop {
        unsafe {
            asm!("wfe", options(nomem, nostack));
        }
    }
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = writeln!(Console, "bootloader: error: {info}");
    bootloader_halt()
}
