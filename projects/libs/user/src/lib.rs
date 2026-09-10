#![no_std]
//! Typed userspace API. Register assignments and syscall numbers stay private.
use core::fmt::{self, Write};
use kernel_abi as abi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidArgument,
    InvalidCapability,
    Unsupported,
    Range,
    Alignment,
    FailedLookup,
    TruncatedMessage,
    DeleteFirst,
    RevokeFirst,
    NoMemory,
    Unknown(u64),
}
fn error(label: u64) -> Error {
    match label {
        abi::INVALID_ARGUMENT => Error::InvalidArgument,
        abi::INVALID_CAPABILITY => Error::InvalidCapability,
        abi::UNSUPPORTED => Error::Unsupported,
        abi::RANGE_ERROR => Error::Range,
        abi::ALIGNMENT_ERROR => Error::Alignment,
        abi::NOT_FOUND => Error::FailedLookup,
        abi::TRUNCATED_MESSAGE => Error::TruncatedMessage,
        abi::ALREADY_MAPPED => Error::DeleteFirst,
        abi::REVOKE_FIRST => Error::RevokeFirst,
        abi::NO_MEMORY => Error::NoMemory,
        code => Error::Unknown(code),
    }
}
fn ipc_address() -> usize {
    let address: usize;
    // Kernel runtime convention; the register is read-only at EL0.
    unsafe {
        core::arch::asm!("mrs {address}, tpidrro_el0", address = out(reg) address, options(nomem, nostack))
    };
    address
}
fn invoke(cap: u64, label: u64, args: &[u64], caps: &[u64]) -> Result<u64, Error> {
    if args.len() > abi::MAX_MESSAGE_WORDS || caps.len() > abi::MAX_EXTRA_CAPS {
        return Err(Error::TruncatedMessage);
    }
    if args.len() > 4 || !caps.is_empty() {
        let address = ipc_address();
        if address == 0 {
            return Err(Error::TruncatedMessage);
        }
        // The task's runtime owns this IPC buffer; no Rust reference to it is
        // exposed. No same-address-space threads or signal reentry exist yet.
        unsafe {
            let buffer = address as *mut abi::IpcBuffer;
            for (index, &word) in args.iter().enumerate().skip(4) {
                core::ptr::addr_of_mut!((*buffer).msg)
                    .cast::<u64>()
                    .add(index)
                    .write_volatile(word);
            }
            for (index, &cap) in caps.iter().enumerate() {
                core::ptr::addr_of_mut!((*buffer).caps_or_badges)
                    .cast::<u64>()
                    .add(index)
                    .write_volatile(cap);
            }
        }
    }
    let mut tag = abi::MessageInfo::new(label, caps.len(), args.len()).word();
    let mut mr0 = args.first().copied().unwrap_or(0);
    let mut mr1 = args.get(1).copied().unwrap_or(0);
    let mut mr2 = args.get(2).copied().unwrap_or(0);
    let mut mr3 = args.get(3).copied().unwrap_or(0);
    unsafe {
        core::arch::asm!("svc #0",
            in("x7") abi::Syscall::Call as i64 as u64,
            inlateout("x0") cap => _,
            inlateout("x1") tag,
            inlateout("x2") mr0,
            inlateout("x3") mr1,
            inlateout("x4") mr2,
            inlateout("x5") mr3);
    }
    let _ = (mr1, mr2, mr3);
    let label = abi::MessageInfo::from_word(tag).label();
    if label == abi::OK {
        Ok(mr0)
    } else {
        Err(error(label))
    }
}
fn runtime(method: abi::RuntimeInvocation, args: &[u64]) -> Result<u64, Error> {
    invoke(abi::INIT_RUNTIME, method as u64, args, &[])
}

pub mod capability;
pub mod elf;
pub mod ipc;
pub mod task;
pub mod thread;
pub use task::{Permissions, Task, TaskState};

/// Sleep for at least the requested milliseconds; zero yields to ready tasks.
pub fn sleep(milliseconds: u64) -> Result<(), Error> {
    runtime(abi::RuntimeInvocation::Sleep, &[milliseconds]).map(|_| ())
}
pub fn clock_milliseconds() -> Result<u64, Error> {
    runtime(abi::RuntimeInvocation::Clock, &[])
}
pub fn available_frames() -> Result<usize, Error> {
    runtime(abi::RuntimeInvocation::AvailableFrames, &[]).map(|n| n as usize)
}
/// Exit and publish a completion value. Object capabilities govern final reclamation.
pub fn exit(code: u64) -> ! {
    let _ = runtime(abi::RuntimeInvocation::Exit, &[code]);
    loop {
        core::hint::spin_loop();
    }
}

pub fn yield_now() -> Result<(), Error> {
    unsafe {
        core::arch::asm!("svc #0", in("x7") abi::Syscall::Yield as i64 as u64);
    }
    Ok(())
}

/// Permanently park this call site. Use Task::suspend for resumable suspension.
pub fn suspend_self() -> ! {
    let current = runtime(abi::RuntimeInvocation::Current, &[]).unwrap_or(abi::INIT_TCB);
    let _ = invoke(current, abi::Invocation::TcbSuspend as u64, &[], &[]);
    // Do not recurse into panic if a broken kernel unexpectedly returns.
    loop {
        core::hint::spin_loop();
    }
}

/// Temporary kernel debug console, unavailable when the kernel uses LOG=off.
pub fn debug_putchar(byte: u8) -> Result<(), Error> {
    if runtime(abi::RuntimeInvocation::DebugConsoleAvailable, &[])? == 0 {
        return Err(Error::Unsupported);
    }
    unsafe {
        core::arch::asm!("svc #0", in("x7") abi::Syscall::DebugPutchar as i64 as u64, in("x0") byte as u64);
    }
    Ok(())
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
