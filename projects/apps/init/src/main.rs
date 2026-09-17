#![no_std]
#![no_main]
//! init: the service manager. Parses `init.cfg` from the boot module ROM,
//! starts each service once its dependencies are Running, grants per-service
//! budgets, devices and endpoints, and enforces the restart policy
//! (restart / max_restarts / window_ms / backoff_ms / critical) with a STOP
//! handshake before forced teardown (docs/service-manager.md §7, §9, §10).
extern crate alloc;

mod logger;
mod policy;
mod spawn;
mod state;

use alloc::vec::Vec;
use core::fmt::Write as _;
use rstiny::thread::ThreadGroup;
use rstiny::{Task, capability::*, ipc};
use rstiny_protocol::{Argument, SpawnInfo, control};
#[allow(unused_imports)]
use state::*;
#[allow(unused_imports)]
use policy::{apply_policy, stop_and_reap, detach_dependents, fail_reason};
#[allow(unused_imports)]
use spawn::{find_module, map_rom, spawn_service};
use rstiny_runtime::entry;

#[global_allocator]
static HEAP: rstiny_alloc::Heap = rstiny_alloc::Heap;

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
            if device_master(device).is_none() {
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
    // IRQHandler masters userboot granted: master i is window slot i, and the
    // window holds `count` slots (docs/irq.md §8). 0 disables IRQ grants.
    let irq_master_base = info.extra[SpawnInfo::IRQ_SLOT];
    let irq_line_count = info.extra[SpawnInfo::IRQ_SLOT + 1];
    for (index, cfg) in config.services.iter().enumerate() {
        // Resolve configured device names to per-service copy slots up front;
        // names were validated against DEVICE_NAMES above.
        let mut device_copies = [0u64; MAX_DEVICES];
        for (k, _name) in cfg.devices.iter().enumerate().take(MAX_DEVICES) {
            device_copies[k] = DEV_COPY_BASE + index as u64 * 8 + k as u64;
        }
        let mut irq_masters = [0u64; MAX_DEVICE_IRQS];
        for (k, master) in service_irq_masters(cfg, irq_master_base, irq_line_count)
            .into_iter()
            .enumerate()
        {
            irq_masters[k] = master;
        }
        let mut irq_copies = [0u64; MAX_DEVICE_IRQS];
        for (k, copy) in irq_copies.iter_mut().enumerate() {
            *copy = IRQ_COPY_BASE + index as u64 * 8 + k as u64;
        }
        services.push(ServiceState {
            cfg: cfg.clone(),
            status: Status::Waiting,
            task: None,
            svc_ep_obj: SVC_EP_BASE + index as u64 * 8,
            budget_slot: SUB_UNTYPED_BASE + index as u64 * 8,
            device_copies,
            irq_masters,
            irq_copies,
            restart_times: Vec::new(),
            restarts: 0,
        });
    }

    // All per-service infrastructure exists before anything runs and
    // survives every restart (docs/service-manager.md §6). Budgets are
    // carved *first*: large alignments (2 MiB, 4 MiB) must run back to back
    // inside init's 16 MiB grant — a 64 byte endpoint landing on an
    // alignment boundary between them pushes every later budget to the next
    // boundary and out of the region (docs/gui-display.md §10).
    for service in &services {
        if let Err(error) = Untyped(CPtr(INIT_UNTYPED)).retype(
            ObjectType::Untyped,
            u64::from(service.cfg.budget_bits),
            cnode.0,
            service.budget_slot,
            1,
        ) {
            rstiny::debug_println!(
                "[init] budget {} ({} bits) failed: {:?}",
                service.cfg.name,
                service.cfg.budget_bits,
                error
            );
            fail_reason(&info, 19);
        }
    }
    for service in &services {
        if let Err(error) = cnode.retype_endpoint(CPtr(INIT_UNTYPED), service.svc_ep_obj) {
            rstiny::debug_println!("[init] ep {} failed: {error:?}", service.cfg.name);
            fail_reason(&info, 19);
        }
        for (k, name) in service.cfg.devices.iter().enumerate().take(MAX_DEVICES) {
            let Some(master) = device_master(name) else {
                fail_reason(&info, 17);
            };
            if let Err(error) = cnode.copy(
                service.device_copies[k],
                CPtr(INIT_CNODE),
                DEV_MASTER_BASE + master,
                RIGHTS_ALL,
            ) {
                rstiny::debug_println!(
                    "[init] devcopy {} ({name}) failed: {error:?}",
                    service.cfg.name
                );
                fail_reason(&info, 19);
            }
        }
        for k in 0..MAX_DEVICE_IRQS {
            if service.irq_masters[k] != 0
                && cnode
                    .copy(
                        service.irq_copies[k],
                        CPtr(INIT_CNODE),
                        service.irq_masters[k],
                        RIGHTS_ALL,
                    )
                    .is_err()
            {
                rstiny::debug_println!(
                    "[init] irqcopy {} failed: master={}",
                    service.cfg.name,
                    service.irq_masters[k]
                );
                fail_reason(&info, 19);
            }
        }
    }

    // Level-1 report: the service manager is configured and supervising.
    let _ = ipc::call(info.control_ep, control::READY, &[]);

    let console_ep = CONSOLE_EP_OWN;
    let mut console_running = false;
    let mut drill_armed = false;
    let mut logger_drill_armed = false;
    // Crash drill (KILL_FS=1): once the full chain is up (appmgr READY), the
    // supervisor reaps fs and detaches its dependents so the whole
    // notify/rebuild path runs (docs/disk-driver.md section 12, D5).
    let kill_fs = option_env!("KILL_FS").is_some_and(|value| value == "1");
    let mut drilled = false;
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
            spawn_service(&mut services, index, console_ep, rom);
        }
        if services.iter().all(|s| matches!(s.status, Status::Failed)) && !services.is_empty() {
            fail_reason(&info, 20);
        }

        let Ok(received) = ipc::recv(CONTROL_OBJ) else {
            rstiny::debug_println!("[init][t] recv err");
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
                let is_console = services[index].cfg.name == CONSOLE;
                let drill = drill_enabled && is_console && services[index].restarts == 0;
                // KILL_FS crash drill (its own build flag, independent of the
                // supervision drill): reply, let appmgr spawn and announce
                // hello, then reap fs and watch dependents restart.
                if kill_fs && !drilled && services[index].cfg.name == "appmgr" {
                    drilled = true;
                    let _ = ipc::reply(0, &[]);
                    // Let the freshly loaded app announce itself first.
                    let _ = rstiny::sleep(5_000);
                    if let Some(fs_index) = services.iter().position(|s| s.cfg.name == "fs") {
                        // The logger posts at every log level; the marker
                        // must be visible with LOG=off too.
                        post_log(console_running, 0, "[init] crash drill", "reaping fs");
                        stop_and_reap(&mut services[fs_index], false);
                        detach_dependents(&mut services, fs_index);
                        apply_policy(&info, &mut services, fs_index, console_running, false, true);
                        continue;
                    }
                }
                if !drill_enabled {
                    // The "service started" log precedes the reply: the woken
                    // service cannot print until its READY call is answered,
                    // so the boot log reads in causal order. The console flag
                    // flips first so the console's own READY logs through the
                    // console service (the debug fallback is silent at
                    // LOG=off). Drill builds keep the original sequence —
                    // their crash write rides the log post *after* the reply
                    // (fault-handler.md §10).
                    if is_console {
                        console_running = true;
                    }
                    post_log(
                        console_running,
                        0,
                        "[init] service started",
                        &services[index].cfg.name,
                    );
                    let _ = ipc::reply(0, &[]);
                    continue;
                }
                let _ = ipc::reply(0, &[]);
                // Drill builds keep the original reply-then-log sequence (the
                // crash write rides the log post) and the settle delay before
                // it: without the delay the woken service races this thread's
                // log post and the boot deadlocks before its first print
                // (kernel follow-up: the reply wakeup vs concurrent-call race).
                let _ = rstiny::sleep(100);
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
                // A lost dependency invalidates the clients above it: notify
                // them (best effort) and restart them so they re-BIND to the
                // replacement service (docs/disk-driver.md section 9).
                detach_dependents(&mut services, index);
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

