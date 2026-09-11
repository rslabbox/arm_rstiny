#![no_std]
//! Minimal service runtime: registration, supervised liveness and logging.
//!
//! A service parses its SpawnInfo page (the x0 start argument), announces
//! READY on the control endpoint and then serves its protocol. The log!
//! macro forwards formatted bytes to the console service through IPC.

use rstiny::ipc::{self, Received};
use rstiny_protocol::{SpawnInfo, console, control};

/// Everything a service needs from its supervisor.
#[derive(Clone, Copy)]
pub struct Service {
    pub control_ep: u64,
    pub command_ep: u64,
    pub console_ep: u64,
    /// Extra endpoint/capability slots granted through `SpawnInfo::extra`.
    pub extra: [u64; 12],
}

impl Service {
    /// Parse the SpawnInfo page at `argument` and announce readiness.
    pub fn init(argument: usize) -> Option<Self> {
        if argument == 0 || argument % protocol_page() != 0 {
            return None;
        }
        // SAFETY: the supervisor mapped this page read-only for the child;
        // only this task reads it, and it stays mapped for the task lifetime.
        let info = unsafe { &*(argument as *const SpawnInfo) };
        if info.magic != SpawnInfo::MAGIC || info.version != SpawnInfo::VERSION {
            return None;
        }
        let service = Self {
            control_ep: info.control_ep,
            command_ep: info.command_ep,
            console_ep: info.extra[SpawnInfo::CONSOLE_EP],
            extra: info.extra,
        };
        ipc::call(service.control_ep, control::READY, &[]).ok()?;
        Some(service)
    }

    /// Report a status code to the supervisor.
    pub fn report(&self, status: u64) {
        let _ = ipc::call(self.control_ep, control::REPORT, &[status]);
    }

    /// Report a normal exit and park; the supervisor decides what happens.
    pub fn exit(&self, code: u64) -> ! {
        let _ = ipc::call(self.control_ep, control::EXIT, &[code]);
        loop {
            core::hint::spin_loop();
        }
    }

    /// Non-blocking STOP poll: acknowledges and returns `true` when asked to
    /// stop. PING is answered inline.
    pub fn poll_stop(&self) -> bool {
        if self.command_ep == 0 {
            return false;
        }
        match ipc::nbrecv(self.command_ep) {
            Ok(Some(received)) if received.label == control::STOP => {
                let _ = ipc::reply(control::STOP_ACK, &[]);
                true
            }
            Ok(Some(received)) if received.label == control::PING => {
                let _ = ipc::reply(0, &[]);
                false
            }
            _ => false,
        }
    }

    /// Forward bytes to the console service; best effort, chunked.
    pub fn log_bytes(&self, bytes: &[u8]) {
        if self.console_ep == 0 {
            return;
        }
        for chunk in bytes.chunks(console::MAX_WRITE) {
            let mut words = [0u64; 16];
            words[0] = chunk.len() as u64;
            for (index, byte) in chunk.iter().enumerate() {
                words[1 + index / 8] |= (*byte as u64) << (8 * (index % 8));
            }
            let length = 1 + chunk.len().div_ceil(8);
            let _ = ipc::call(self.console_ep, console::WRITE, &words[..length]);
        }
    }
}

fn protocol_page() -> usize {
    rstiny_protocol::PAGE_SIZE as usize
}

/// Format a line and forward it to the console service.
#[macro_export]
macro_rules! log {
    ($service:expr, $($arg:tt)*) => {{
        let service: &$crate::Service = &$service;
        let mut buffer = [0u8; 256];
        let used = {
            let mut writer = $crate::LineWriter { buffer: &mut buffer, used: 0 };
            let _ = ::core::fmt::Write::write_fmt(&mut writer, ::core::format_args!($($arg)*));
            writer.used
        };
        service.log_bytes(&buffer[..used]);
    }};
}
#[macro_export]
macro_rules! logln {
    ($service:expr) => {
        $crate::log!($service, "\n")
    };
    ($service:expr, $($arg:tt)*) => {{
        let service: &$crate::Service = &$service;
        let mut buffer = [0u8; 256];
        let used = {
            let mut writer = $crate::LineWriter { buffer: &mut buffer, used: 0 };
            let _ = ::core::fmt::Write::write_fmt(&mut writer, ::core::format_args!($($arg)*));
            writer.used
        };
        // Always terminate the line: reserving the last byte truncates a
        // too-long message rather than dropping its newline.
        let used = used.min(buffer.len() - 1);
        buffer[used] = b'\n';
        service.log_bytes(&buffer[..used + 1]);
    }};
}

/// Write into a caller-owned line buffer.
pub struct LineWriter<'a> {
    pub buffer: &'a mut [u8],
    pub used: usize,
}
impl core::fmt::Write for LineWriter<'_> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let remaining = self.buffer.len() - self.used;
        let count = text.len().min(remaining);
        self.buffer[self.used..self.used + count].copy_from_slice(&text.as_bytes()[..count]);
        self.used += count;
        Ok(())
    }
}

/// Build a `Service` from a raw SpawnInfo page without announcing readiness.
/// Supervisors use this to inspect their own parameter page.
pub fn parse_info(argument: usize) -> Option<SpawnInfo> {
    if argument == 0 || argument % protocol_page() != 0 {
        return None;
    }
    // SAFETY: supervisor-provided, read-only, task-lifetime mapping.
    let info = unsafe { core::ptr::read(argument as *const SpawnInfo) };
    (info.magic == SpawnInfo::MAGIC && info.version == SpawnInfo::VERSION).then_some(info)
}

/// Deliver a fault message's summary fields for supervisor logging.
pub fn fault_summary(received: &Received) -> (u64, u64, u64) {
    (received.word(0), received.word(1), received.word(2))
}
