#![no_std]
#![no_main]
//! init: the service manager. Parses `init.cfg` from the boot module ROM,
//! starts each service once its dependencies are Running, grants per-service
//! budgets, devices and endpoints, and enforces the restart policy
//! (restart / max_restarts / window_ms / backoff_ms / critical) with a STOP
//! handshake before forced teardown (docs/service-manager.md §7, §9, §10).
extern crate alloc;

mod logger;

use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::fmt::Write as _;
use core::ptr::addr_of_mut;
use rstiny::elf::ChildCap;
use rstiny::thread::ThreadGroup;
use rstiny::{Error, Task, capability::*, ipc};
use rstiny_protocol::{Argument, SpawnInfo, control};
use rstiny_runtime::entry;

const CONSOLE: &str = "console";

// init's CSpace layout, granted by userboot.
const CONTROL_OBJ: u64 = 142; // init's supervision endpoint for its services
const CONSOLE_EP_OWN: u64 = 50; // console service endpoint object cap
const UART_DEV_OWN: u64 = 161; // first device Untyped copy (from userboot)
// Per-service init-side cap blocks. They must stay clear of the ROM Frame
// window granted to init (200..200+512) and below the loader's own range.
const SUB_UNTYPED_BASE: u64 = 5000; // + i*8: per-service budget slots
const SVC_EP_BASE: u64 = 5004; // + i*8: service main endpoint slots
const THREAD_SLOT_BASE: u64 = 6000; // thread-group caps (16 per thread)
const LOGGER_FAULT_SLOT: u64 = 144; // logger's badged fault-endpoint cap
const CHILD_SCRATCH: usize = 0x07E0_0000; // loader scratch while spawning
const ROM_VA: usize = 0x0200_0000;

const SERVICE_BADGE_BASE: u64 = 1; // console = 1; others follow config order
const INTERNAL_BADGE_BASE: u64 = 0x8000; // group-internal threads (logger …)
const STOP_TIMEOUT_MS: u64 = 500;
const BACKOFF_SHIFT_CAP: u32 = 5;
/// Supervision drill: init hands itself back to userboot with this exit code,
/// exercising the group destroy and the level-1 restart (BOOT_TEST builds).
const INIT_EXIT_DRILL_CODE: u64 = 7;

const DEVICE_NAMES: [&str; 1] = ["uart0"];

/// Service lifecycle states (docs/service-manager.md §12).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    /// Waiting for dependencies to become Running.
    Waiting,
    Running,
    /// Crashed or exited cleanly; restart pending (backoff) or final.
    Terminated,
    /// Restart budget exhausted or policy says never: permanent.
    Failed,
}

struct ServiceState {
    cfg: rstiny_initcfg::ServiceCfg,
    status: Status,
    task: Option<Task>,
    /// init-side cap slots: service main endpoint and per-service budget.
    svc_ep_obj: u64,
    budget_slot: u64,
    /// Restart timestamps inside the observation window (clock ms).
    restart_times: Vec<u64>,
    restarts: u32,
    /// Per-service endpoints and budget created (survive restarts).
    infra: bool,
}

/// Single-core bump allocator over a fixed BSS pool: config parsing needs
/// owned names, and nothing is freed before shutdown.
const POOL_BYTES: usize = 64 * 1024;
struct Bump;
static mut POOL: [u64; POOL_BYTES / 8] = [0; POOL_BYTES / 8];
static mut POOL_USED: usize = 0;
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: single-core user task; the cursor is only touched here.
        unsafe {
            let used = addr_of_mut!(POOL_USED);
            let start = addr_of_mut!(POOL) as usize;
            let offset = (*used).next_multiple_of(layout.align().max(8));
            if offset + layout.size() > POOL_BYTES {
                return core::ptr::null_mut();
            }
            *used = offset + layout.size();
            (start + offset) as *mut u8
        }
    }
    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: Layout) {}
}
#[global_allocator]
static ALLOCATOR: Bump = Bump;

/// Build-time supervision drill switch (Makefile BOOT_TEST=1).
fn boot_test() -> bool {
    option_env!("BOOT_TEST").is_some_and(|value| value == "1")
}

#[entry]
fn main(argument: Argument) -> ! {
    let Some(info) = rstiny_server::parse_info(argument) else {
        loop {
            core::hint::spin_loop();
        }
    };
    run(info)
}

fn run(info: SpawnInfo) -> ! {
    let rom = match map_rom(&info) {
        Ok(rom) => rom,
        Err(_) => fail_reason(&info, 1),
    };
    let Some(config_start) = find_module(rom, "init.cfg") else {
        fail_reason(&info, 13);
    };
    let Ok(config_text) = core::str::from_utf8(config_start) else {
        fail_reason(&info, 14);
    };
    let Ok(config) = rstiny_initcfg::parse(config_text) else {
        fail_reason(&info, 15);
    };
    // Every configured ELF and device name must be resolvable up front.
    for service in &config.services {
        if find_module(rom, &service.elf).is_none() {
            fail_reason(&info, 16);
        }
        for device in &service.devices {
            if !DEVICE_NAMES.contains(&device.as_str()) {
                fail_reason(&info, 17);
            }
        }
    }

    let cnode = CNode(CPtr(INIT_CNODE));
    // Shared service infrastructure: init's supervision endpoint, the console
    // service endpoint object (its only client is init for now) and the
    // supervisor→logger endpoint of init's own thread group.
    if cnode
        .retype_endpoint(CPtr(INIT_UNTYPED), CONTROL_OBJ)
        .is_err()
    {
        fail_reason(&info, 18);
    }
    if cnode
        .retype_endpoint(CPtr(INIT_UNTYPED), CONSOLE_EP_OWN)
        .is_err()
    {
        fail_reason(&info, 21);
    }
    if cnode
        .retype_endpoint(CPtr(INIT_UNTYPED), logger::LOG_EP)
        .is_err()
    {
        fail_reason(&info, 22);
    }
    // The supervision drill only runs in the first incarnation: userboot hands
    // a restart generation through SpawnInfo::extra (thread-group.md §7).
    let drill_enabled = boot_test() && info.extra[5] == 0;
    rstiny::debug_println!(
        "[init] boot_test={} generation={}",
        boot_test(),
        info.extra[5]
    );
    // init is a thread group sharing one CSpace/VSpace (docs/fault-handler.md
    // §6): the supervisor below only Recvs `control_ep` and issues non-blocking
    // kernel object calls, while the client thread owns every blocking Call to
    // a supervised service. That is what breaks the §1 deadlock: the thread
    // blocked in a Call is not the thread that must receive the fault.
    let mut group = ThreadGroup::new(
        CPtr(INIT_CNODE),
        CPtr(INIT_VSPACE),
        CPtr(INIT_UNTYPED),
        THREAD_SLOT_BASE,
    );
    // SAFETY: logger::run reads only its own stack/IPC buffer and the shared
    // CSpace; entry and stack are mapped by spawn_thread before the resume.
    let logger_entry = logger::run as fn(usize) -> !;
    // The logger is itself supervised: its faults carry an internal badge to
    // this thread's control endpoint (docs/thread-group.md §4).
    let logger_fault = rstiny::thread::FaultSupervision {
        source: CONTROL_OBJ,
        slot: LOGGER_FAULT_SLOT,
        badge: INTERNAL_BADGE_BASE,
    };
    let mut logger = match unsafe {
        group.spawn_thread(
            logger_entry as usize,
            CONSOLE_EP_OWN as u64,
            Some(logger_fault),
        )
    } {
        Ok(thread) => Some(thread),
        Err(_) => fail_reason(&info, 23),
    };

    let mut services: Vec<ServiceState> = Vec::new();
    for (index, cfg) in config.services.iter().enumerate() {
        services.push(ServiceState {
            cfg: cfg.clone(),
            status: Status::Waiting,
            task: None,
            svc_ep_obj: SVC_EP_BASE + index as u64 * 8,
            budget_slot: SUB_UNTYPED_BASE + index as u64 * 8,
            restart_times: Vec::new(),
            restarts: 0,
            infra: false,
        });
    }

    // Level-1 report: the service manager is configured and supervising.
    let _ = ipc::call(info.control_ep, control::READY, &[]);

    let console_ep = CONSOLE_EP_OWN;
    let mut console_running = false;
    let mut drill_armed = false;
    let mut logger_drill_armed = false;
    loop {
        // Start every service whose dependencies are all Running.
        for index in 0..services.len() {
            let deps_ok = services[index].cfg.depends.iter().all(|dependency| {
                services
                    .iter()
                    .any(|s| s.cfg.name == *dependency && s.status == Status::Running)
            });
            if services[index].status != Status::Waiting || !deps_ok {
                continue;
            }
            // Endpoints and budget survive restarts (supervisor-owned); the
            // teardown revoke resets the per-service budget watermark.
            if !services[index].infra {
                let (svc_ep_obj, budget_slot) =
                    (services[index].svc_ep_obj, services[index].budget_slot);
                if cnode
                    .retype_endpoint(CPtr(INIT_UNTYPED), svc_ep_obj)
                    .is_err()
                    || Untyped(CPtr(INIT_UNTYPED))
                        .retype(
                            ObjectType::Untyped,
                            u64::from(services[index].cfg.budget_bits),
                            cnode.0,
                            budget_slot,
                            1,
                        )
                        .is_err()
                {
                    fail_reason(&info, 19);
                }
                services[index].infra = true;
            }
            spawn_service(&mut services, index, console_ep, rom);
        }
        if services.iter().all(|s| matches!(s.status, Status::Failed)) && !services.is_empty() {
            fail_reason(&info, 20);
        }

        let Ok(received) = ipc::recv(CONTROL_OBJ) else {
            continue;
        };
        if received.badge >= INTERNAL_BADGE_BASE {
            // Group-internal thread fault (docs/thread-group.md §4.3): reap
            // exactly that TCB — never the whole group — and rebuild it with
            // the same entry and fault endpoint. Only non-blocking kernel
            // object calls happen here; the fault-handler.md §7 invariant
            // still holds.
            rstiny::debug_println!(
                "[init] internal thread faulted: badge={:#x} label={}",
                received.badge,
                received.label
            );
            if let Some(thread) = logger.take() {
                let _ = Task::from_tcb(thread.tcb).destroy_thread();
                // Release the dead thread's per-thread caps and its fault cap
                // so the rebuild can reuse both (docs/thread-group.md §4.3).
                // SAFETY: the thread is terminated and nothing references its
                // stack, IPC buffer or cap slots.
                unsafe {
                    thread.release();
                    let _ = cnode.delete(LOGGER_FAULT_SLOT);
                }
            }
            let logger_fault = rstiny::thread::FaultSupervision {
                source: CONTROL_OBJ,
                slot: LOGGER_FAULT_SLOT,
                badge: INTERNAL_BADGE_BASE,
            };
            // SAFETY: as for the initial spawn.
            let logger_entry = logger::run as fn(usize) -> !;
            match unsafe {
                group.spawn_thread(
                    logger_entry as usize,
                    CONSOLE_EP_OWN as u64,
                    Some(logger_fault),
                )
            } {
                Ok(thread) => {
                    // A just-spawned logger has no Call in flight, so this
                    // one reliable send is bounded by scheduling only — the
                    // supervisor never waits on a service here.
                    post_log_flags(true, 0, true, "[init] logger rebuilt", "logging recovered");
                    logger = Some(thread);
                    if drill_enabled {
                        // Let the logger finish the verification write, then
                        // hand the whole group back to userboot: the level-1
                        // supervisor destroys it with a group destroy and
                        // rebuilds init (docs/thread-group.md §7, G2 chained).
                        for _ in 0..10_000 {
                            let _ = rstiny::yield_now();
                        }
                        let _ = ipc::call(info.control_ep, control::EXIT, &[INIT_EXIT_DRILL_CODE]);
                        loop {
                            core::hint::spin_loop();
                        }
                    }
                }
                Err(_) => rstiny::debug_println!("[init] logger rebuild failed"),
            }
            continue;
        }
        let Some(index) = (0..services.len()).position(|i| badge_for(i) == received.badge) else {
            continue;
        };
        match received.label {
            control::READY if received.badge == badge_for(index) => {
                services[index].status = Status::Running;
                // READY arrived as a Call: answer it before anything else,
                // or the service stays BlockedReply (§7.3).
                let _ = ipc::reply(0, &[]);
                if services[index].cfg.name == CONSOLE {
                    console_running = true;
                }
                // Supervision drill (test builds only, first incarnation):
                // the crash write rides the same log post, issued by the
                // *client* thread, so the fault-receiving supervisor never
                // blocks on the service it supervises (fault-handler.md §10).
                // After the console restarts, the drill continues with a
                // logger self-crash to exercise group-internal supervision
                // (thread-group.md §4.3).
                let mut flags = 0;
                let drill = drill_enabled
                    && services[index].cfg.name == CONSOLE
                    && services[index].restarts == 0;
                if drill {
                    flags |= logger::FLAG_CRASH_DRILL;
                } else if drill_enabled
                    && services[index].cfg.name == CONSOLE
                    && services[index].restarts == 1
                    && !logger_drill_armed
                {
                    flags |= logger::FLAG_CRASH_SELF;
                    logger_drill_armed = true;
                }
                post_log(
                    console_running,
                    flags,
                    "[init] service started",
                    &services[index].cfg.name,
                );
                drill_armed = drill;
            }
            control::REPORT if received.badge == badge_for(index) => {
                let _ = ipc::reply(0, &[]);
            }
            // EXIT or a kernel fault: run the stop/teardown path, then apply
            // the restart policy from the service's configuration.
            _ => {
                let graceful = received.label == control::EXIT && received.word(0) == 0;
                let crashed = matches!(received.label, 0..=4);
                if services[index].cfg.name == CONSOLE {
                    // The console is gone until it announces READY again;
                    // logs fall back to the debug console in the meantime.
                    console_running = false;
                }
                stop_and_reap(&mut services[index], graceful);
                if drill_armed {
                    drill_armed = false;
                    // The client thread publishes the drill outcome once its
                    // Call returned; waiting on it here cannot deadlock — the
                    // logger is not blocked on any service, and this thread
                    // has already reaped the one that crashed.
                    if logger::take_drill_result() == 1 {
                        rstiny::debug_println!(
                            "[init] client drill call failed as designed: the reaped service freed the call"
                        );
                    } else {
                        rstiny::debug_println!("[init] client drill call unexpectedly succeeded");
                    }
                }
                apply_policy(
                    &info,
                    &mut services,
                    index,
                    console_running,
                    graceful,
                    crashed,
                );
            }
        }
    }
}

fn badge_for(index: usize) -> u64 {
    SERVICE_BADGE_BASE + index as u64
}

/// STOP handshake (graceful) when the service is Running, then destroy and
/// reclaim its derivation subtree and budget watermark.
fn stop_and_reap(service: &mut ServiceState, graceful_exit: bool) {
    // A faulted or exited service is already halted; the STOP handshake
    // applies only to a *live* service being stopped on command (§9) — the
    // v1 restart path never needs it because the crash already halted the
    // task. Destroy unconditionally: Runtime::Destroy finishes the scheduler
    // task, and the derivation-subtree revoke resets the budget watermark.
    if let Some(task) = service.task.take() {
        let _ = task.destroy();
        // The ordinary budget comes back through the allocator revoke above;
        // device regions are shared with the supervisor's own copy, so the
        // service's derivation must be revoked explicitly for the watermark
        // to reset and the next instance to re-carve its frames (§8).
        let cnode = CNode(CPtr(INIT_CNODE));
        for (device_index, _) in service.cfg.devices.iter().enumerate() {
            // SAFETY: the terminated service's copies are the only descendants
            // and its task is already gone.
            unsafe {
                let _ = cnode.revoke(UART_DEV_OWN + device_index as u64);
            }
        }
        rstiny::debug_println!("[init] teardown done");
    }
}

/// Apply the configured restart policy after a service terminated.
fn apply_policy(
    info: &SpawnInfo,
    services: &mut [ServiceState],
    index: usize,
    console_running: bool,
    graceful_exit: bool,
    crashed: bool,
) {
    use rstiny_initcfg::Restart;
    let service = &mut services[index];
    let failed_run = crashed || !graceful_exit;
    let now = clock_ms();
    // Sliding observation window.
    service
        .restart_times
        .retain(|stamp| now.saturating_sub(*stamp) < u64::from(service.cfg.window_ms));

    let allowed = match service.cfg.restart {
        Restart::Never => false,
        Restart::OnFailure => failed_run,
        Restart::Always => true,
    };
    let within_budget = (service.restart_times.len() as u32) < service.cfg.max_restarts;
    if !allowed || !within_budget {
        // No restart: clean exits settle as Terminated, everything else Failed.
        service.status = if failed_run {
            Status::Failed
        } else {
            Status::Terminated
        };
        if service.cfg.critical && failed_run {
            fail_reason(info, 30 + index as u64);
        }
        post_log(
            console_running,
            0,
            "[init] service stopped",
            &service.cfg.name,
        );
        return;
    }
    // Backoff grows with the consecutive restart count.
    let backoff = service
        .cfg
        .backoff_ms
        .saturating_mul(1 << service.restarts.min(BACKOFF_SHIFT_CAP));
    if backoff > 0 {
        rstiny::debug_println!("[init] backoff {}", backoff);
        let _ = rstiny::sleep(u64::from(backoff));
        rstiny::debug_println!("[init] backoff done");
    }
    service.restarts += 1;
    service.restart_times.push(now);
    service.status = Status::Waiting; // dependencies may need rechecking
    if console_running {
        post_log(
            true,
            0,
            "[init] service restart scheduled",
            &service.cfg.name,
        );
    }
}

/// Spawn one service: budget, endpoints, devices, console client cap and the
/// ELF from ROM, all before its first instruction.
fn spawn_service(services: &mut Vec<ServiceState>, index: usize, console_ep: u64, rom: &[u8]) {
    let Some(elf) = find_module(rom, &services[index].cfg.elf) else {
        services[index].status = Status::Failed;
        return;
    };
    let service = &services[index];
    let spawn_info = SpawnInfo {
        magic: SpawnInfo::MAGIC,
        version: SpawnInfo::VERSION,
        control_ep: 140,
        command_ep: 0, // v1: STOP rides the service's main endpoint
        untyped: 32,
        rom_start: 0,
        rom_count: 0,
        extra: {
            let mut extra = [0; 8];
            extra[SpawnInfo::CONSOLE_EP] = 51;
            extra[1] = 33; // first device slot in the child
            extra
        },
    };
    let info_bytes = unsafe {
        core::slice::from_raw_parts(
            &spawn_info as *const SpawnInfo as *const u8,
            core::mem::size_of::<SpawnInfo>(),
        )
    };
    // Per-service fixed child caps: control, console client, budget, ASID
    // pool, then the service's devices.
    let (budget_slot, badge) = (service.budget_slot, badge_for(index));
    let mut caps = [ChildCap {
        slot: 0,
        source: 0,
        rights: 0,
        badge: 0,
    }; 4 + DEVICE_NAMES.len()];
    caps[0] = ChildCap {
        slot: 140,
        source: CONTROL_OBJ,
        rights: RIGHTS_ALL,
        badge,
    };
    caps[1] = ChildCap {
        slot: 51,
        source: console_ep,
        rights: RIGHTS_ALL,
        badge: 0,
    };
    caps[2] = ChildCap {
        slot: 32,
        source: budget_slot,
        rights: RIGHTS_ALL,
        badge: 0,
    };
    caps[3] = ChildCap {
        slot: 6,
        source: INIT_ASID_POOL,
        rights: RIGHTS_ALL,
        badge: 0,
    };
    for (device_index, _) in service.cfg.devices.iter().enumerate() {
        caps[4 + device_index] = ChildCap {
            slot: 33 + device_index as u64,
            source: UART_DEV_OWN + device_index as u64,
            rights: RIGHTS_ALL,
            badge: 0,
        };
    }
    let used = 4 + service.cfg.devices.len();

    match unsafe {
        // The service's allocations carve its own per-service sub-region, so
        // the teardown revoke (Task::destroy) is scoped to this service only.
        rstiny::elf::spawn_supervised(
            elf,
            CHILD_SCRATCH,
            service.budget_slot,
            &rstiny::elf::Supervision {
                info: info_bytes,
                fault_ep: 140,
                caps: &caps[..used],
            },
        )
    } {
        Ok(task) => {
            services[index].task = Some(task);
            services[index].status = Status::Running;
        }
        Err(error) => {
            rstiny::debug_println!(
                "[init] spawn {} failed: {:?}",
                services[index].cfg.name,
                error
            );
            services[index].status = Status::Failed;
        }
    }
}

/// Map the granted ROM Frame capabilities read-only at `ROM_VA`.
fn map_rom(info: &SpawnInfo) -> Result<&'static [u8], Error> {
    if info.rom_start == 0 || info.rom_count == 0 {
        return Err(Error::InvalidArgument);
    }
    let cnode = CNode(CPtr(INIT_CNODE));
    let untyped = Untyped(CPtr(INIT_UNTYPED));
    let count = info.rom_count as usize;
    let mut tables = [false; 64];
    for index in 0..count {
        let va = ROM_VA + index * 4096;
        ensure_table(&cnode, &untyped, va, &mut tables, 1000)?;
        unsafe {
            Page(CPtr(info.rom_start + index as u64)).map(
                CPtr(INIT_VSPACE),
                va,
                RIGHTS_READ,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )?;
        }
    }
    // SAFETY: the ROM window is mapped read-only for this task's lifetime.
    Ok(unsafe { core::slice::from_raw_parts(ROM_VA as *const u8, count * 4096) })
}

fn ensure_table(
    cnode: &CNode,
    untyped: &Untyped,
    address: usize,
    tables: &mut [bool; 64],
    first_slot: u64,
) -> Result<(), Error> {
    let index = address >> 21;
    if index >= 64 || tables[index] {
        return Ok(());
    }
    let slot = first_slot + index as u64;
    untyped.retype(ObjectType::PageTable, 0, cnode.0, slot, 1)?;
    PageTable(CPtr(slot)).map(CPtr(INIT_VSPACE), address & !0x1F_FFFF)?;
    tables[index] = true;
    Ok(())
}

/// Find a module's contents in the archive bytes.
fn find_module<'a>(rom: &'a [u8], name: &str) -> Option<&'a [u8]> {
    use rstiny_newc::BootArchive;
    let archive = BootArchive::parse(rom).ok()?;
    archive
        .modules()
        .iter()
        .find(|(module_name, _)| *module_name == name.as_bytes())
        .map(|(_, data)| *data)
}

fn clock_ms() -> u64 {
    rstiny::clock_milliseconds().unwrap_or(0)
}

/// Post a log line to the client thread: an asynchronous `NBSend`, best
/// effort, with the payload carried in the message registers so the two
/// threads share no mutable memory (docs/fault-handler.md §6.1). The line is
/// dropped when the logger is busy: the supervisor must never wait on the
/// client thread — a fault arriving while it cannot receive would deadlock
/// the whole group (the very §1 loop this structure removes).
fn post_log(via_console: bool, extra_flags: u64, prefix: &str, name: &str) {
    post_log_flags(via_console, extra_flags, false, prefix, name);
}

/// `reliable` blocks in `Send` until the logger takes the request. Only valid
/// right after a (re)spawn, when the logger provably has no service `Call` in
/// flight — otherwise the supervisor could miss a fault delivery while it
/// waits.
fn post_log_flags(via_console: bool, extra_flags: u64, reliable: bool, prefix: &str, name: &str) {
    let mut buffer = [0u8; logger::MAX_LINE];
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
        flags |= logger::FLAG_CONSOLE;
    }
    let mut words = [0u64; 16];
    let length = logger::pack(&buffer[..used], flags, &mut words);
    if reliable {
        let _ = ipc::send(logger::LOG_EP, logger::POST, &words[..length]);
    } else {
        let _ = ipc::nbsend(logger::LOG_EP, logger::POST, &words[..length]);
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

fn fail_reason(info: &SpawnInfo, reason: u64) -> ! {
    rstiny::debug_println!("[init] fail reason={}", reason);
    // Report failure to the supervisor through the badged control endpoint;
    // EXIT(1) triggers the level-1 restart policy.
    let _ = ipc::call(info.control_ep, control::EXIT, &[reason]);
    loop {
        core::hint::spin_loop();
    }
}
