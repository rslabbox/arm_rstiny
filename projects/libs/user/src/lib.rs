#![no_std]
//! Typed userspace API. Register assignments and syscall numbers stay private.
use core::fmt::{self, Write};
use kernel_abi as abi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidArgument,
    NoMemory,
    NotMapped,
    AlreadyMapped,
    PermissionDenied,
    NotFound,
    InvalidState,
    Busy,
    Unknown(u64),
}

fn invoke(number: u64, args: [u64; 5]) -> Result<u64, Error> {
    let status: u64;
    let value: u64;
    // The kernel preserves x2..x30; x0 is status and x1 is the result for
    // value-returning calls. Memory may change across this boundary.
    unsafe {
        core::arch::asm!("svc #0", in("x8") number,
            inlateout("x0") args[0] => status, inlateout("x1") args[1] => value,
            in("x2") args[2], in("x3") args[3], in("x4") args[4]);
    }
    match status {
        abi::OK => Ok(value),
        abi::UNSUPPORTED => Err(Error::Unsupported),
        abi::INVALID_ARGUMENT => Err(Error::InvalidArgument),
        abi::NO_MEMORY => Err(Error::NoMemory),
        abi::NOT_MAPPED => Err(Error::NotMapped),
        abi::ALREADY_MAPPED => Err(Error::AlreadyMapped),
        abi::PERMISSION_DENIED => Err(Error::PermissionDenied),
        abi::NOT_FOUND => Err(Error::NotFound),
        abi::INVALID_STATE => Err(Error::InvalidState),
        abi::BUSY => Err(Error::Busy),
        code => Err(Error::Unknown(code)),
    }
}
fn call(number: u64, argument: u64) -> Result<(), Error> {
    invoke(number, [argument, 0, 0, 0, 0]).map(|_| ())
}

pub mod elf;
pub mod task;
pub use task::{Permissions, Task, TaskState};

/// Sleep for at least the requested milliseconds; zero yields to ready tasks.
pub fn sleep(milliseconds: u64) -> Result<(), Error> {
    call(abi::SYS_SLEEP, milliseconds)
}
pub fn clock_milliseconds() -> Result<u64, Error> {
    invoke(abi::SYS_CLOCK, [0; 5])
}
pub fn available_frames() -> Result<usize, Error> {
    invoke(abi::SYS_MEMORY_AVAILABLE, [0; 5]).map(|n| n as usize)
}
/// Exit, releasing the current address space. Its parent can wait and reap it.
pub fn exit(code: u64) -> ! {
    let _ = call(abi::SYS_EXIT, code);
    loop {
        core::hint::spin_loop();
    }
}

pub fn yield_now() -> Result<(), Error> {
    call(abi::SYS_YIELD, 0)
}

/// Permanently park this call site. Use Task::suspend for resumable suspension.
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
