//! Restart policy: STOP handshake, teardown, backoff and restart budgets
//! (docs/service-manager.md §9, §10).

use rstiny::capability::*;
use rstiny::ipc;
use rstiny::{Task};
use rstiny_protocol::{control, SpawnInfo};

use crate::state::*;

pub fn detach_dependents(services: &mut [ServiceState], index: usize) {
    let name = services[index].cfg.name.clone();
    for j in 0..services.len() {
        if j == index || !services[j].cfg.depends.iter().any(|d| *d == name) {
            continue;
        }
        // Best-effort notice on the dependent's own endpoint; the restart
        // below is the guarantee, the notice is the graceful path.
        let _ = ipc::nbsend(services[j].svc_ep_obj, control::DEPENDENCY_LOST, &[]);
        if matches!(services[j].status, Status::Starting | Status::Running) {
            stop_and_reap(&mut services[j], false);
            let now = clock_ms();
            let window = u64::from(services[j].cfg.window_ms);
            services[j]
                .restart_times
                .retain(|stamp| now.saturating_sub(*stamp) < window);
            services[j].restarts += 1;
            services[j].restart_times.push(now);
            services[j].status = if services[j].restarts > services[j].cfg.max_restarts {
                Status::Failed
            } else {
                Status::Waiting
            };
        }
    }
}

pub fn stop_and_reap(service: &mut ServiceState, _graceful_exit: bool) {
    // The device Untyped copies are supervisor-owned and survive the
    // teardown, like the budget: revoking the copy itself would finalise the
    // *whole device region* — every other VirtIO driver's window frames with
    // it (kernel `finalise_untyped` is region-granular, and block- and
    // gpu-server share one window, docs/gui-display.md §2). Nothing here may
    // revoke a device region.
    let cnode = CNode(CPtr(INIT_CNODE));
    // Quiesce the service's IRQ lines through the masters: Clear drops the
    // (possibly dead) notification binding and disables/deactivates the line
    // so a re-authorized driver starts deliverable (docs/irq.md §8).
    for k in 0..MAX_DEVICE_IRQS {
        if service.irq_masters[k] != 0 {
            let _ = IrqHandler(CPtr(service.irq_masters[k])).clear();
            // SAFETY: the terminated service no longer touches the device.
            unsafe {
                let _ = cnode.revoke(service.irq_copies[k]);
            }
        }
    }
    // A faulted or exited service is already halted; the STOP handshake
    // applies only to a *live* service being stopped on command (§9) — the
    // v1 restart path never needs it because the crash already halted the
    // task. Destroy unconditionally: Runtime::Destroy finishes the scheduler
    // task, and the derivation-subtree revoke resets the budget watermark.
    if let Some(task) = service.task.take() {
        let _ = task.destroy();
        rstiny::debug_println!("[init] teardown done");
    }
}

/// Apply the configured restart policy after a service terminated.
pub fn apply_policy(
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
pub fn fail_reason(info: &SpawnInfo, reason: u64) -> ! {
    rstiny::debug_println!("[init] fail reason={}", reason);
    // Report failure to the supervisor through the badged control endpoint;
    // EXIT(1) triggers the level-1 restart policy.
    let _ = ipc::call(info.control_ep, control::EXIT, &[reason]);
    loop {
        core::hint::spin_loop();
    }
}

