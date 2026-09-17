//! Post-mortem helpers: frame-pointer backtraces and the emergency raw
//! writer used on the panic path (IRQs masked, logger lock possibly held —
//! output goes straight to the polling UART).
//!
//! Frame pointers are forced for the aarch64 target in `.cargo/config.toml`;
//! without them the kernel chain stops after one frame.

use crate::{memory::AddressSpace, utils::console};

/// Emergency raw output: no logger, no lock, no level filter.
fn print_str(text: &str) {
    for byte in text.bytes() {
        let _ = console::put_byte(byte);
    }
}

/// Walk the current kernel stack's frame-pointer chain (x29), printing each
/// caller's return address. Bounded and self-validating: any non-kernel or
/// misaligned frame ends the walk. Symbolize with tools/symbolize.py.
pub fn kernel_backtrace() {
    print_str("kernel backtrace (fp chain):\n");
    let mut fp: u64;
    // SAFETY: reads the current frame pointer register.
    unsafe { core::arch::asm!("mov {}, x29", out(reg) fp) };
    for depth in 0..16u32 {
        if fp == 0 || fp % 16 != 0 || fp >> 48 != 0xffff {
            print_str("  (chain end)\n");
            return;
        }
        // SAFETY: the kernel stack is mapped writable and only this core
        // touches it while IRQs are masked.
        let lr = unsafe { ((fp + 8) as *const u64).read_volatile() };
        let next = unsafe { (fp as *const u64).read_volatile() };
        let _ = write_frame(depth, lr, fp);
        if next <= fp || next >> 48 != 0xffff {
            print_str("  (chain end)\n");
            return;
        }
        fp = next;
    }
}

/// Walk a user task's frame-pointer chain: `vspace` translates each probe
/// read, so a torn-down address space simply ends the walk.
pub fn user_backtrace(space: &AddressSpace, frame: &crate::arch::kernel::thread::TrapFrame) {
    print_str("user backtrace (fp chain):\n");
    let _ = write_frame(0, frame.elr, 0);
    let mut fp = frame.r[29] as usize;
    for depth in 1..8u32 {
        if fp == 0 || fp % 16 != 0 {
            break;
        }
        let mut word = [0u8; 8];
        if space.read(fp, &mut word).is_err() {
            print_str("  (unmapped frame)\n");
            return;
        }
        let lr = u64::from_le_bytes(word);
        let _ = write_frame(depth, lr, fp as u64);
        fp = fp + 16;
    }
}

fn write_frame(depth: u32, address: u64, extra: u64) -> core::fmt::Result {
    use core::fmt::Write as _;
    let mut buffer = [0u8; 64];
    let mut writer = FrameWriter { data: &mut buffer, used: 0 };
    write!(writer, "  #{depth}: lr={address:#x} fp={extra:#x}\n")
}

struct FrameWriter<'a> {
    data: &'a mut [u8],
    used: usize,
}
impl core::fmt::Write for FrameWriter<'_> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.push_bytes(text.as_bytes());
        Ok(())
    }
}
impl FrameWriter<'_> {
    fn push_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.used < self.data.len() {
                self.data[self.used] = byte;
                self.used += 1;
            }
        }
    }
}

/// Panic path: everything below runs with IRQs masked and must not re-enter
/// the logger.
pub fn panic_dump() {
    kernel_backtrace();
    let mut sink = |text: &str| print_str(text);
    crate::task::api::dump_tasks_raw(&mut sink);
}
