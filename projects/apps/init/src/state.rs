//! init's CSpace layout constants, per-service supervision state and the
//! log/clock helpers the supervision loop is built from.

extern crate alloc;

use core::fmt::Write as _;
use alloc::vec::Vec;
use rstiny::{ipc, Task};

pub const CONSOLE: &str = "console";

// init's CSpace layout, granted by userboot.
pub const CONTROL_OBJ: u64 = 142; // init's supervision endpoint for its services
pub const CONSOLE_EP_OWN: u64 = 50; // console service endpoint object cap
pub const DEV_MASTER_BASE: u64 = 161;
pub const DEV_COPY_BASE: u64 = 170; // + i*8 + k: per-service device Untyped copies // device Untyped masters, +k per DEVICE_NAMES[k]
// Per-service init-side cap blocks. They must stay clear of the ROM Frame
// window granted to init (200..200+512) and below the loader's own range.
pub const SUB_UNTYPED_BASE: u64 = 5000; // + i*8: per-service budget slots
pub const SVC_EP_BASE: u64 = 5004; // + i*8: service main endpoint slots
pub const IRQ_COPY_BASE: u64 = 5002; // + i*8: per-service device IRQHandler copies
pub const THREAD_SLOT_BASE: u64 = 6000; // thread-group caps (16 per thread)
pub const LOGGER_FAULT_SLOT: u64 = 144; // logger's badged fault-endpoint cap
/// Device *frames* carved once from the device masters and granted to the
/// drivers as Frame caps (docs/gui-display.md §10): the UART region is one
/// page, the VirtIO window is four (16 KiB). The drivers never retype device
/// memory, so a driver restart cannot touch another driver's window.

pub const CHILD_SCRATCH: usize = 0x07E0_0000; // loader scratch while spawning
pub const ROM_VA: usize = 0x0200_0000;

// Child CSpace layout handed to every spawned service.
pub const CHILD_CONTROL: u64 = 140;
pub const CHILD_CONSOLE: u64 = 51;
pub const CHILD_BUDGET: u64 = 32;
pub const CHILD_DEV_BASE: u64 = 33;
pub const CHILD_SELF_EP: u64 = 52;
pub const CHILD_DEP_BASE: u64 = 53;
pub const CHILD_IRQ: u64 = 56;
pub const CHILD_IRQ2: u64 = 57;
pub const MAX_DEVICES: usize = 4;
pub const MAX_DEPS: usize = 3;
/// Device IRQ handlers one service may drive (gpu-server: gpu + keyboard).
pub const MAX_DEVICE_IRQS: usize = 2;

pub const SERVICE_BADGE_BASE: u64 = 1; // console = 1; others follow config order
pub const INTERNAL_BADGE_BASE: u64 = 0x8000; // group-internal threads (logger …)
pub const _STOP_TIMEOUT_MS: u64 = 500;
pub const BACKOFF_SHIFT_CAP: u32 = 5;
/// Supervision drill: init hands itself back to userboot with this exit code,
/// exercising the group destroy and the level-1 restart (BOOT_TEST builds).
pub const INIT_EXIT_DRILL_CODE: u64 = 7;

/// Service lifecycle states (docs/service-manager.md §12).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Waiting for dependencies to become Running.
    Waiting,
    /// Spawned, before its READY announcement.
    Starting,
    Running,
    /// Crashed or exited cleanly; restart pending (backoff) or final.
    Terminated,
    /// Restart budget exhausted or policy says never: permanent.
    Failed,
}

pub struct ServiceState {
    pub cfg: rstiny_initcfg::ServiceCfg,
    pub status: Status,
    pub task: Option<Task>,
    /// init-side cap slots: service main endpoint and per-service budget.
    pub svc_ep_obj: u64,
    pub budget_slot: u64,
    /// Per-service device Untyped copies (init-side, survive restarts).
    pub device_copies: [u64; MAX_DEVICES],
    /// The service's device IRQ lines: init-side master slots and per-service
    /// copy slots (0 = no interrupt for that device). One master per device
    /// the configuration names, in order (docs/gui-display.md §2).
    pub irq_masters: [u64; MAX_DEVICE_IRQS],
    pub irq_copies: [u64; MAX_DEVICE_IRQS],
    /// Restart timestamps inside the observation window (clock ms).
    pub restart_times: Vec<u64>,
    pub restarts: u32,
}

/// Task heap: rstiny-alloc (interpreter-app.md 决策 B) — the shared dual-
/// language allocator. Config parsing and service bookkeeping allocate from
/// the bootstrap pool; growth retypes from init's own Untyped budget.

/// Build-time supervision drill switch (Makefile BOOT_TEST=1).
pub fn boot_test() -> bool {
    option_env!("BOOT_TEST").is_some_and(|value| value == "1")
}



pub fn badge_for(index: usize) -> u64 {
    SERVICE_BADGE_BASE + index as u64
}

/// The VirtIO slot line a configured device name refers to. "virtio-mmio-N"
/// is the Nth VirtIO device in the granted window; QEMU attaches devices to
/// the window's mmio transports from its END, so device N sits in slot
/// `count-1-N` and uses that slot's line (fixed-platform contract verified
/// against `probe`, docs/irq.md §7).
pub fn virtio_slot(name: &str, line_count: u64) -> Option<u64> {
    name.strip_prefix("virtio-mmio-")
        .and_then(|suffix| suffix.parse::<u64>().ok())
        .filter(|&device| device < line_count)
        .map(|device| line_count - 1 - device)
}

/// The device Untyped master a configured name resolves to. Device names
/// resolve to the masters userboot granted in ascending physical order
/// (docs/disk-driver.md section 5.1): master 0 is the PL011, master 1 the
/// *whole* VirtIO MMIO window. `virtio-mmio-N` names the Nth VirtIO device
/// inside that one window (docs/gui-display.md §2): every ordinal shares
/// master 1, only the IRQ line differs.
pub fn device_master(name: &str) -> Option<u64> {
    match name {
        "uart0" => Some(0),
        other => other
            .strip_prefix("virtio-mmio-")?
            .parse::<u64>()
            .ok()
            .map(|_| 1),
    }
}

/// The init-side IRQHandler master slots serving `cfg`'s devices, in
/// configuration order: one line per named VirtIO device (docs/gui-display.md
/// §2), up to [`MAX_DEVICE_IRQS`].
pub fn service_irq_masters(
    cfg: &rstiny_initcfg::ServiceCfg,
    master_base: u64,
    line_count: u64,
) -> [u64; MAX_DEVICE_IRQS] {
    let mut masters = [0u64; MAX_DEVICE_IRQS];
    if master_base == 0 {
        return masters;
    }
    for (k, slot) in cfg
        .devices
        .iter()
        .filter_map(|name| virtio_slot(name, line_count))
        .take(MAX_DEVICE_IRQS)
        .enumerate()
    {
        masters[k] = master_base + slot;
    }
    masters
}

/// STOP handshake (graceful) when the service is Running, then destroy and
/// reclaim its derivation subtree and budget watermark.


pub fn clock_ms() -> u64 {
    rstiny::clock_milliseconds().unwrap_or(0)
}

/// Post a log line to the client thread: an asynchronous `NBSend`, best
/// effort, with the payload carried in the message registers so the two
/// threads share no mutable memory (docs/fault-handler.md §6.1). The line is
/// dropped when the logger is busy: the supervisor must never wait on the
/// client thread — a fault arriving while it cannot receive would deadlock
/// the whole group (the very §1 loop this structure removes).
pub fn post_log(via_console: bool, extra_flags: u64, prefix: &str, name: &str) {
    post_log_flags(via_console, extra_flags, false, prefix, name);
}

/// `reliable` blocks in `Send` until the logger takes the request. Only valid
/// right after a (re)spawn, when the logger provably has no service `Call` in
/// flight — otherwise the supervisor could miss a fault delivery while it
/// waits.
pub fn post_log_flags(via_console: bool, extra_flags: u64, reliable: bool, prefix: &str, name: &str) {
    let mut buffer = [0u8; crate::logger::MAX_LINE];
    let used = {
        let mut writer = LineWriter {
            buffer: &mut buffer,
            used: 0,
        };
        let _ = writer.write_str(prefix);
        let _ = writer.write_str(": ");
        let _ = writer.write_str(name);
        writer.used
    };
    let mut flags = extra_flags;
    if via_console && CONSOLE_EP_OWN != 0 {
        flags |= crate::logger::FLAG_CONSOLE;
    }
    let mut words = [0u64; 16];
    let length = crate::logger::pack(&buffer[..used], flags, &mut words);
    if reliable {
        let _ = ipc::send(crate::logger::LOG_EP, crate::logger::POST, &words[..length]);
    } else {
        let _ = ipc::nbsend(crate::logger::LOG_EP, crate::logger::POST, &words[..length]);
    }
}

/// Write into a caller-owned line buffer.
struct LineWriter<'a> {
    buffer: &'a mut [u8],
    used: usize,
}
impl core::fmt::Write for LineWriter<'_> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let remaining = self.buffer.len() - self.used;
        let count = text.len().min(remaining);
        self.buffer[self.used..self.used + count].copy_from_slice(text.as_bytes());
        self.used += count;
        Ok(())
    }
}

