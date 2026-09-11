//! init's client/logger thread (docs/fault-handler.md §5, §6).
//!
//! The supervisor thread is the only receiver on `control_ep`; this thread is
//! the only executor that issues blocking `ipc::call`s to supervised services
//! (console writes). Both share init's CSpace and VSpace, but the logger owns
//! its stack, IPC buffer and register state — so a crashed or stuck service
//! can never keep the supervisor from receiving the fault that would free it.

use rstiny::debug_println;
use rstiny::ipc::{self, Received};
use rstiny_protocol::console;

/// Supervisor→logger endpoint slot in init's shared CSpace.
pub const LOG_EP: u64 = 143;
/// Internal logger label, in its own protocol segment
/// (`rstiny_protocol::internal`, docs/service-manager.md decision 14).
pub use rstiny_protocol::internal::POST;
/// Deliver the line through the console service instead of the debug console.
pub const FLAG_CONSOLE: u64 = 1;
/// Perform the supervision-drill crash write after the line is delivered.
pub const FLAG_CRASH_DRILL: u64 = 2;
/// Trigger this thread's own fault after the line is delivered: the drill for
/// group-internal fault supervision (docs/thread-group.md §4.3).
pub const FLAG_CRASH_SELF: u64 = 4;
/// Console's crash trigger (projects/apps/console: `CRASH_MAGIC`).
const CRASH_MAGIC: u64 = 0xCAFE_BABE;
/// Reply label of a successful call. A failed call surfaces as a reply
/// message carrying the kernel's error label (`ipc::call` always yields
/// `Ok(Received)`; the label is the error channel).
const REPLY_OK: u64 = 0;
/// Maximum line length: two header words plus twelve payload words ride the
/// message registers, so the supervisor and the logger share no mutable memory.
pub const MAX_LINE: usize = 12 * 8;

/// Body of the client/logger thread. `argument` is the console service
/// endpoint slot; the supervisor hands it over at spawn time because the
/// logger resolves the same shared CSpace.
pub fn run(argument: usize) -> ! {
    let console_ep = argument as u64;
    loop {
        let Ok(request) = ipc::recv(LOG_EP) else {
            continue;
        };
        let count = (request.word(0) as usize).min(MAX_LINE);
        let flags = request.word(1);
        let line = unpack(&request, count);
        if flags & FLAG_CONSOLE != 0 {
            if let Err(error) = write_console(console_ep, &line[..count]) {
                // Console unavailable: fall back to the kernel debug console.
                debug_println!("[init][client:{:?}] {}", error, line_str(&line, count));
            }
        } else {
            debug_println!("[init] {}", line_str(&line, count));
        }
        if flags & FLAG_CRASH_DRILL != 0 {
            // Invariant under test (docs/fault-handler.md §7.1, §10): this
            // blocking Call is issued by the client thread, never by the
            // fault-receiving supervisor. When the service faults, the
            // supervisor reaps it and this Call fails with a kernel error
            // label instead of blocking forever. The outcome is published in
            // shared memory (same VSpace, single writer) and reported by the
            // supervisor, so the evidence line is never garbled by concurrent
            // debug-console writes.
            let drill = [CRASH_MAGIC];
            let failed = match ipc::call(console_ep, console::WRITE, &drill) {
                Ok(received) => received.label != REPLY_OK,
                Err(_) => true,
            };
            store_drill_result(u32::from(failed));
        }
        if flags & FLAG_CRASH_SELF != 0 {
            // Deliberate crash so the drill can observe the full supervision
            // chain: fault → internal badge → reap → rebuild (§4.3). The read
            // result is consumed so nothing is optimised away.
            let boom = unsafe { core::ptr::read_volatile(core::ptr::null::<u64>()) };
            core::hint::black_box(boom);
        }
    }
}

/// Drill outcome published by the client thread: 0 = pending, 1 = the Call
/// failed as designed, 2 = it unexpectedly succeeded.
static mut DRILL_RESULT: u32 = 0;

fn store_drill_result(value: u32) {
    // SAFETY: single core; only the client thread writes and only the
    // supervisor reads this location.
    unsafe {
        core::ptr::addr_of_mut!(DRILL_RESULT).write_volatile(value);
    }
}

/// Supervisor side: poll the outcome, yielding so the client thread can run.
/// Returns 0 when the client never reported (bounded wait).
pub fn take_drill_result() -> u32 {
    for _ in 0..1_000_000 {
        // SAFETY: as above; the read resets nothing, the client writes 0/1/2.
        let value = unsafe { core::ptr::addr_of!(DRILL_RESULT).read_volatile() };
        if value != 0 {
            store_drill_result(0);
            return value;
        }
        let _ = rstiny::yield_now();
    }
    0
}

/// Blocking console write, chunked to the protocol's inline limit. The
/// service answers with label 0 and the written byte count; anything else
/// (including the kernel's error reply after the service was reaped) is an
/// error.
fn write_console(console_ep: u64, bytes: &[u8]) -> Result<(), u64> {
    for chunk in bytes.chunks(console::MAX_WRITE) {
        let received = match ipc::call(console_ep, console::WRITE, &pack_words(chunk)) {
            Ok(received) => received,
            Err(_) => return Err(u64::MAX),
        };
        if received.label != REPLY_OK {
            return Err(received.label);
        }
    }
    // Log lines are newline-terminated; the console service turns LF into CRLF.
    // (The debug-console path terminates via `debug_println!` instead.)
    match ipc::call(console_ep, console::WRITE, &pack_words(b"\n")) {
        Ok(received) if received.label == REPLY_OK => Ok(()),
        Ok(received) => Err(received.label),
        Err(_) => Err(u64::MAX),
    }
}

/// Pack `bytes` little-endian behind the byte count, per the console wire
/// format (docs/service-manager.md §14.2).
fn pack_words(bytes: &[u8]) -> [u64; 16] {
    let mut words = [0u64; 16];
    words[0] = bytes.len() as u64;
    for (index, byte) in bytes.iter().enumerate() {
        words[1 + index / 8] |= (*byte as u64) << (8 * (index % 8));
    }
    words
}

/// Pack a log line into a POST message: `[len, flags, payload…]`.
pub fn pack(line: &[u8], flags: u64, words: &mut [u64; 16]) -> usize {
    let count = line.len().min(MAX_LINE);
    words[0] = count as u64;
    words[1] = flags;
    for (index, byte) in line[..count].iter().enumerate() {
        words[2 + index / 8] |= (*byte as u64) << (8 * (index % 8));
    }
    2 + count.div_ceil(8)
}

fn unpack(request: &Received, count: usize) -> [u8; MAX_LINE] {
    let mut line = [0u8; MAX_LINE];
    for index in 0..count {
        line[index] = (request.word(2 + index / 8) >> (8 * (index % 8))) as u8;
    }
    line
}

fn line_str(line: &[u8], count: usize) -> &str {
    core::str::from_utf8(&line[..count]).unwrap_or("<binary log>")
}
