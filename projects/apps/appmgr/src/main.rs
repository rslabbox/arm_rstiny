#![no_std]
#![no_main]
//! appmgr: the application manager (docs/disk-driver.md section 10). Reads
//! `APPS.CFG` from the FAT32 disk through the fs service, loads each listed
//! ELF from the same disk and spawns it under supervision — replacing a file
//! on the disk changes what runs, without rebuilding the system image.

extern crate alloc;

use alloc::vec::Vec;
use core::hint::spin_loop;

use rstiny::Task;
use rstiny::capability::{
    CNode, CPtr, INIT_ASID_POOL, INIT_CNODE, INIT_UNTYPED, INIT_VSPACE, ObjectType, Page,
    PageTable, RIGHTS_ALL, RIGHTS_READ, RIGHTS_WRITE, Untyped, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
use rstiny::elf::{ChildCap, LOADER_SLOT_BASE, LOADER_SLOT_STRIDE, Supervision};
use rstiny::ipc;
use rstiny_initcfg::Restart;
use rstiny_protocol::{Argument, SpawnInfo, control, fs, status};
use rstiny_runtime::entry;
use rstiny_server::{Service, logln};

const WINDOW_VA: usize = 0x0400_0000; // fs shared buffer (received on BIND)
const TABLE_SLOT: u64 = 44;
const RECV_SLOT: u64 = 60; // landing slot for the fs BIND cap transfer
const APP_SLOT_BASE: u64 = 70; // per-app Untyped budget, +k*8
const APP_SCRATCH: usize = 0x07E0_0000;
const CHILD_CONTROL: u64 = 140;
const CHILD_CONSOLE: u64 = 51;
const CHILD_BUDGET: u64 = 32;
const MAX_APPS: usize = 4;
const ELF_MAX: usize = 256 * 1024;
const BACKOFF_SHIFT_CAP: u32 = 5;

/// Bump allocator: app ELFs and manifest strings live here; freed only when
/// the supervisor tears the task down.
const POOL_BYTES: usize = 512 * 1024;
struct Bump;
static mut POOL: [u64; POOL_BYTES / 8] = [0; POOL_BYTES / 8];
static mut POOL_USED: usize = 0;
unsafe impl alloc::alloc::GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: alloc::alloc::Layout) -> *mut u8 {
        // SAFETY: single-core user task; the cursor is only touched here.
        unsafe {
            let used = core::ptr::addr_of_mut!(POOL_USED);
            let start = core::ptr::addr_of_mut!(POOL) as usize;
            let offset = (*used).next_multiple_of(layout.align().max(8));
            if offset + layout.size() > POOL_BYTES {
                return core::ptr::null_mut();
            }
            *used = offset + layout.size();
            (start + offset) as *mut u8
        }
    }
    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: alloc::alloc::Layout) {}
}
#[global_allocator]
static ALLOCATOR: Bump = Bump;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AppState {
    Starting,
    Running,
    Terminated,
    Failed,
}
struct App {
    cfg: rstiny_initcfg::ServiceCfg,
    state: AppState,
    task: Option<Task>,
    budget_slot: u64,
    restarts: u32,
    restart_times: Vec<u64>,
}

/// fs client helpers: data lands in the shared buffer mapped at WINDOW_VA.
fn fs_call(fs_ep: u64, label: u64, words: &[u64]) -> Option<ipc::Received> {
    let received = ipc::call(fs_ep, label, words).ok()?;
    (received.label == status::OK).then_some(received)
}

fn fs_name_words(name: &[u8]) -> Option<Vec<u64>> {
    if name.is_empty() || name.len() > 16 {
        return None;
    }
    let mut words = Vec::new();
    words.push(name.len() as u64);
    words.push(0);
    words.push(0);
    for (index, byte) in name.iter().enumerate() {
        words[1 + index / 8] |= u64::from(*byte) << (8 * (index % 8));
    }
    Some(words)
}

fn fs_open(fs_ep: u64, name: &[u8]) -> Option<(u64, u64)> {
    let words = fs_name_words(name)?;
    let received = fs_call(fs_ep, fs::OPEN, &words)?;
    Some((received.word(0), received.word(1)))
}

/// Read a whole file through the fs service into a heap buffer.
fn fs_read_all(fs_ep: u64, file_id: u64, size: u64) -> Option<Vec<u8>> {
    if size > ELF_MAX as u64 {
        return None;
    }
    let mut data = Vec::new();
    let mut offset = 0;
    while offset < size {
        let length = (size - offset).min(0x1000) as u64;
        let received = fs_call(fs_ep, fs::READ, &[file_id, offset, length])?;
        let read = received.word(0) as usize;
        if read == 0 {
            return None;
        }
        // SAFETY: the fs shared buffer is exclusively mapped by this task and
        // only written while this task is blocked in the READ call.
        let window = unsafe { core::slice::from_raw_parts(WINDOW_VA as *const u8, read) };
        data.extend_from_slice(&window[..read]);
        offset += read as u64;
    }
    Some(data)
}

fn spawn_app(
    service: &Service,
    index: usize,
    image: &[u8],
    cfg: &rstiny_initcfg::ServiceCfg,
    budget_slot: u64,
) -> Result<Task, rstiny::Error> {
    // One fresh sub-Untyped per (re)start, carved from appmgr's own budget
    // (slot 32): teardown revokes it, so a restart never leaks.
    let cnode = CNode(CPtr(INIT_CNODE));
    Untyped(CPtr(INIT_UNTYPED)).retype(
        ObjectType::Untyped,
        u64::from(cfg.budget_bits),
        cnode.0,
        budget_slot,
        1,
    )?;
    let spawn_info = SpawnInfo {
        magic: SpawnInfo::MAGIC,
        version: SpawnInfo::VERSION,
        control_ep: CHILD_CONTROL,
        command_ep: 0,
        untyped: CHILD_BUDGET,
        rom_start: 0,
        rom_count: 0,
        extra: {
            let mut extra = [0; SpawnInfo::EXTRA_LEN];
            extra[SpawnInfo::CONSOLE_EP] = CHILD_CONSOLE;
            extra
        },
    };
    let info_bytes = unsafe {
        core::slice::from_raw_parts(
            &spawn_info as *const SpawnInfo as *const u8,
            core::mem::size_of::<SpawnInfo>(),
        )
    };
    // The app reports to appmgr's own endpoint, badged per app; the console
    // client cap is copied through; slot 6 is the standard ASID pool.
    let caps = [
        ChildCap {
            slot: CHILD_CONTROL,
            source: service.extra[SpawnInfo::SELF_EP],
            rights: RIGHTS_ALL,
            badge: index as u64 + 1,
        },
        ChildCap {
            slot: CHILD_CONSOLE,
            source: service.console_ep,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: CHILD_BUDGET,
            source: budget_slot,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: 6,
            source: INIT_ASID_POOL,
            rights: RIGHTS_ALL,
            badge: 0,
        },
    ];
    unsafe {
        rstiny::elf::spawn_supervised(
            image,
            APP_SCRATCH,
            budget_slot,
            &Supervision {
                info: info_bytes,
                fault_ep: CHILD_CONTROL,
                caps: &caps,
                slot_base: LOADER_SLOT_BASE + index as u64 * LOADER_SLOT_STRIDE,
            },
        )
    }
}

fn clock_ms() -> u64 {
    rstiny::clock_milliseconds().unwrap_or(0)
}

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = Service::init(argument) else {
        loop {
            spin_loop();
        }
    };
    let Some(self_ep) = service
        .extra
        .get(SpawnInfo::SELF_EP)
        .copied()
        .filter(|s| *s != 0)
    else {
        service.exit(2);
    };
    let Some(fs_ep) = service
        .extra
        .get(SpawnInfo::DEP_EP_BASE)
        .copied()
        .filter(|s| *s != 0)
    else {
        service.exit(2);
    };
    let cnode = CNode(CPtr(INIT_CNODE));
    if Untyped(CPtr(INIT_UNTYPED))
        .retype(ObjectType::PageTable, 0, cnode.0, TABLE_SLOT, 1)
        .is_err()
    {
        logln!(service, "[appmgr] cannot budget the covering table");
        service.exit(3);
    }
    if PageTable(CPtr(TABLE_SLOT))
        .map(CPtr(INIT_VSPACE), WINDOW_VA & !0x1F_FFFF)
        .is_err()
    {
        logln!(service, "[appmgr] cannot map the fs window");
        service.exit(3);
    }
    // Receive the fs shared buffer, then read the manifest from the disk.
    if ipc::set_receive_spec(rstiny::ipc::ReceiveSpec {
        cnode: INIT_CNODE,
        index: RECV_SLOT,
        depth: 64,
    })
    .is_err()
    {
        service.exit(3);
    }
    let bind_ok =
        ipc::call_cap(fs_ep, fs::BIND, &[fs::PROTOCOL_VERSION], &[]).is_ok_and(|received| {
            received.label == status::OK && received.word(0) == fs::PROTOCOL_VERSION
        });
    if !bind_ok {
        logln!(service, "[appmgr] fs BIND failed");
        service.exit(5);
    }
    // SAFETY: exclusively mapped by this task.
    unsafe {
        if Page(CPtr(RECV_SLOT))
            .map(
                CPtr(INIT_VSPACE),
                WINDOW_VA,
                RIGHTS_READ | RIGHTS_WRITE,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )
            .is_err()
        {
            logln!(service, "[appmgr] cannot map the fs buffer");
            service.exit(3);
        }
    }
    let Some((manifest_id, manifest_size)) = fs_open(fs_ep, b"APPS.CFG") else {
        logln!(service, "[appmgr] APPS.CFG not found");
        service.exit(6);
    };
    let Some(manifest) = fs_read_all(fs_ep, manifest_id, manifest_size) else {
        logln!(service, "[appmgr] cannot read APPS.CFG");
        service.exit(6);
    };
    let _ = fs_call(fs_ep, fs::CLOSE, &[manifest_id]);
    let Ok(text) = core::str::from_utf8(&manifest) else {
        logln!(service, "[appmgr] APPS.CFG is not UTF-8");
        service.exit(6);
    };
    let Ok(config) = rstiny_initcfg::parse(text) else {
        logln!(service, "[appmgr] APPS.CFG is invalid");
        service.exit(6);
    };
    logln!(
        service,
        "[appmgr] manifest lists {} app(s)",
        config.services.len()
    );
    logln!(
        service,
        "[appmgr] frames={}",
        rstiny::available_frames().unwrap_or(0)
    );

    let mut apps: Vec<App> = Vec::new();
    for (index, cfg) in config.services.iter().enumerate().take(MAX_APPS) {
        apps.push(App {
            cfg: cfg.clone(),
            state: AppState::Starting,
            task: None,
            budget_slot: APP_SLOT_BASE + index as u64 * 8,
            restarts: 0,
            restart_times: Vec::new(),
        });
    }

    // Spawn every app listed in the manifest; ELF bytes come from the disk.
    for index in 0..apps.len() {
        let name = apps[index].cfg.elf.clone();
        let Some((file_id, size)) = fs_open(fs_ep, name.as_bytes()) else {
            logln!(service, "[appmgr] {} not on disk", name);
            apps[index].state = AppState::Failed;
            continue;
        };
        let Some(image) = fs_read_all(fs_ep, file_id, size) else {
            logln!(service, "[appmgr] cannot read {}", name);
            apps[index].state = AppState::Failed;
            continue;
        };
        match spawn_app(
            &service,
            index,
            &image,
            &apps[index].cfg,
            apps[index].budget_slot,
        ) {
            Ok(task) => {
                apps[index].task = Some(task);
                logln!(service, "[appmgr] spawned {}", name);
            }
            Err(error) => {
                logln!(service, "[appmgr] spawn {} failed: {:?}", name, error);
                apps[index].state = AppState::Failed;
            }
        }
    }

    loop {
        let Ok(received) = ipc::recv(self_ep) else {
            continue;
        };
        let Some(index) = (0..apps.len()).position(|i| i as u64 + 1 == received.badge) else {
            continue;
        };
        match received.label {
            control::READY => {
                apps[index].state = AppState::Running;
                // READY arrived as a Call: answer it or the app stays parked.
                let _ = ipc::reply(0, &[]);
                logln!(service, "[appmgr] app started: {}", apps[index].cfg.name);
            }
            control::REPORT => {
                let _ = ipc::reply(0, &[]);
            }
            control::DEPENDENCY_LOST => {
                // The fs service died: file handles and the shared buffer are
                // gone. Stop the apps first (they are our children), then let
                // init rebuild us from scratch.
                logln!(service, "[appmgr] fs lost; stopping apps");
                for app in apps.iter_mut() {
                    if let Some(task) = app.task.take() {
                        let _ = task.destroy();
                    }
                }
                service.exit(9);
            }
            _ => {
                // EXIT or a kernel fault: reap, then apply the restart policy.
                let graceful = received.label == control::EXIT && received.word(0) == 0;
                if let Some(task) = apps[index].task.take() {
                    let _ = task.destroy();
                }
                // SAFETY: the app is gone; its budget derivation subtree is
                // revoked with it, so the retype below starts from zero.
                unsafe {
                    let _ = cnode.revoke(apps[index].budget_slot);
                }
                let now = clock_ms();
                let window = u64::from(apps[index].cfg.window_ms);
                apps[index]
                    .restart_times
                    .retain(|stamp| now.saturating_sub(*stamp) < window);
                let failed_run = !graceful;
                let allowed = match apps[index].cfg.restart {
                    Restart::Never => false,
                    Restart::OnFailure => failed_run,
                    Restart::Always => true,
                };
                let within_budget =
                    (apps[index].restart_times.len() as u32) < apps[index].cfg.max_restarts;
                if !allowed || !within_budget {
                    apps[index].state = if failed_run {
                        AppState::Failed
                    } else {
                        AppState::Terminated
                    };
                    logln!(service, "[appmgr] app stopped: {}", apps[index].cfg.name);
                    continue;
                }
                let backoff = apps[index]
                    .cfg
                    .backoff_ms
                    .saturating_mul(1 << apps[index].restarts.min(BACKOFF_SHIFT_CAP));
                if backoff > 0 {
                    let _ = rstiny::sleep(u64::from(backoff));
                }
                apps[index].restarts += 1;
                apps[index].restart_times.push(now);
                // Reload the ELF from the disk: a replaced file changes what runs.
                let name = apps[index].cfg.elf.clone();
                let Some((file_id, size)) = fs_open(fs_ep, name.as_bytes()) else {
                    apps[index].state = AppState::Failed;
                    continue;
                };
                let Some(image) = fs_read_all(fs_ep, file_id, size) else {
                    apps[index].state = AppState::Failed;
                    continue;
                };
                match spawn_app(
                    &service,
                    index,
                    &image,
                    &apps[index].cfg,
                    apps[index].budget_slot,
                ) {
                    Ok(task) => {
                        apps[index].task = Some(task);
                        apps[index].state = AppState::Starting;
                        logln!(
                            service,
                            "[appmgr] app restart scheduled: {}",
                            apps[index].cfg.name
                        );
                    }
                    Err(_) => apps[index].state = AppState::Failed,
                }
            }
        }
    }
}
