#![no_std]
//! Typed userspace API. Register assignments and syscall numbers stay private.
use core::fmt::{self, Write};
use kernel_abi as abi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidArgument,
    Unknown(u64),
}

fn call(number: u64, argument: u64) -> Result<(), Error> {
    let result: u64;
    // All GPRs except x0 are preserved by the kernel. Do not use nomem:
    // future IPC calls can modify shared memory across this boundary.
    unsafe {
        core::arch::asm!("svc #0", in("x8") number, inlateout("x0") argument => result);
    }
    match result {
        abi::OK => Ok(()),
        abi::UNSUPPORTED => Err(Error::Unsupported),
        abi::INVALID_ARGUMENT => Err(Error::InvalidArgument),
        code => Err(Error::Unknown(code)),
    }
}

pub fn yield_now() -> Result<(), Error> {
    call(abi::SYS_YIELD, 0)
}

/// Suspend the current task. There is currently no resume API.
pub fn suspend_self() -> ! {
    let _ = call(abi::SYS_SUSPEND_SELF, 0);
    // Do not recurse into panic if a broken kernel unexpectedly returns.
    loop {
        core::hint::spin_loop();
    }
}

/// Temporary kernel debug console, unavailable when the kernel uses LOG=off.
pub fn debug_putchar(byte: u8) -> Result<(), Error> {
    call(abi::SYS_DEBUG_PUTCHAR, byte.into())
}

struct DebugWriter;
impl Write for DebugWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            debug_putchar(byte).map_err(|_| fmt::Error)?;
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn debug_print(args: fmt::Arguments<'_>) {
    // Debug output is best effort, including when disabled by LOG=off.
    let _ = DebugWriter.write_fmt(args);
}

#[macro_export]
macro_rules! debug_println {
    () => { $crate::debug_print(core::format_args!("\n")) };
    ($($arg:tt)*) => {
        $crate::debug_print(core::format_args!("{}\n", core::format_args!($($arg)*)))
    };
}
