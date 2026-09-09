#![no_std]
#![no_main]
//! init: the service manager. Parses `init.cfg` from the boot module ROM,
//! starts each service with a private object budget, and restarts failed
//! services per policy (docs/service-manager.md).
use core::{alloc::GlobalAlloc, ptr::addr_of_mut};
use rstiny::elf::ChildCap;
use rstiny::{Error, Task, capability::*, ipc};
use rstiny_protocol::{Argument, SpawnInfo, control};
use rstiny_runtime::entry;
use rstiny_server::{fault_summary, parse_info};

const CONSOLE_ELF: &str = "console.elf";
const CONFIG: &str = "init.cfg";

// init's CSpace layout, granted by userboot.
const CONTROL_EP: u64 = 140; // console's slot for init's control endpoint
const COMMAND_EP: u64 = 141; // console's command endpoint
const CONSOLE_EP_OWN: u64 = 50; // init's client cap for the console protocol
const CONSOLE_EP_CHILD: u64 = 51; // console's service endpoint cap
const UART_DEV_CHILD: u64 = 33; // console's UART device Untyped slot
const CONSOLE_BUDGET: u64 = 32; // console's Untyped budget slot
const CONTROL_OBJ: u64 = 142; // init's control endpoint object (services)
const COMMAND_OBJ: u64 = 143; // init's command endpoint object
const SUB_UNTYPED_SLOT: u64 = 160; // console's budget carved from init's pool
const UART_DEV_OWN: u64 = 161; // UART device Untyped granted by userboot
const ROM_VA: usize = 0x0400_0000;
const CHILD_SCRATCH: usize = 0x07E0_0000;
/// Backstop for the restart guard while `Runtime::Sleep` remains the only
/// timer source (decision 12): a bound on consecutive restart attempts.
const MAX_CONSECUTIVE_RESTARTS: u32 = 20;

const SERVICE_BADGE: u64 = 2;

/// Single-core bump allocator over a fixed BSS pool: init's config parsing
/// needs owned names, and nothing is ever freed before shutdown.
const POOL_BYTES: usize = 64 * 1024;
struct Bump;
static mut POOL: [u64; POOL_BYTES / 8] = [0; POOL_BYTES / 8];
static mut POOL_USED: usize = 0;
use core::alloc::Layout;
unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: single-core, IRQ-masked user task; the static cursor is only
        // touched from this allocator on the one thread of this task.
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

#[entry]
fn main(argument: Argument) -> ! {
    let Some(info) = parse_info(argument) else {
        loop {
            core::hint::spin_loop();
        }
    };
    run(info)
}

fn run(info: SpawnInfo) -> ! {
    let rom = match map_rom(&info) {
        Ok(rom) => rom,
        Err(error) => {
            rstiny::debug_println!("[init] map_rom err: {:?}", error);
            fail(&info)
        }
    };
    let Some(config_start) = find_module(rom, CONFIG) else {
        fail_reason(&info, 13);
    };
    let Ok(config_text) = core::str::from_utf8(config_start) else {
        rstiny::debug_println!("[init] config not utf8");
        fail(&info);
    };
    let Ok(config) = rstiny_initcfg::parse(config_text) else {
        rstiny::debug_println!("[init] config parse failed");
        fail(&info);
    };
    let Some(console_cfg) = config.services.iter().find(|s| s.name == "console") else {
        fail_reason(&info, 10);
    };
    if console_cfg.elf != CONSOLE_ELF {
        fail_reason(&info, 11);
    }
    let Some(console_elf) = find_module(rom, CONSOLE_ELF) else {
        fail_reason(&info, 12);
    };

    // Service infrastructure: endpoints for supervision and the console
    // protocol, plus the service's own Untyped budget carved from ours.
    let cnode = CNode(CPtr(INIT_CNODE));
    let budget = Untyped(CPtr(INIT_UNTYPED));
    if cnode
        .retype_endpoint(CPtr(INIT_UNTYPED), CONTROL_OBJ)
        .is_err()
        || cnode
            .retype_endpoint(CPtr(INIT_UNTYPED), COMMAND_OBJ)
            .is_err()
        || cnode
            .retype_endpoint(CPtr(INIT_UNTYPED), CONSOLE_EP_OWN)
            .is_err()
        || budget
            .retype(ObjectType::Untyped, 22, cnode.0, SUB_UNTYPED_SLOT, 1)
            .is_err()
    {
        fail(&info);
    }

    let mut restarts: u32 = 0;
    let mut running: Option<Task> = None;
    loop {
        if running.is_none() {
            match spawn_console(console_elf) {
                Ok(task) => running = Some(task),
                Err(error) => {
                    rstiny::debug_println!("[init] spawn_console err: {:?}", error);
                    restarts += 1;
                    if restarts > MAX_CONSECUTIVE_RESTARTS {
                        fail(&info);
                    }
                    continue;
                }
            }
        }
        // Service messages arrive on init's own supervision endpoint.
        let Ok(received) = ipc::recv(CONTROL_OBJ) else {
            continue;
        };
        match received.label {
            control::READY if received.badge == SERVICE_BADGE => {
                restarts = 0;
                let _ = ipc::reply(0, &[]);
            }
            control::REPORT if received.badge == SERVICE_BADGE => {
                let _ = ipc::reply(0, &[]);
            }
            // EXIT from the service or a kernel fault delivered on our
            // control endpoint: tear the service down and restart it.
            control::EXIT | 0..=4 => {
                let (pc, far, fault) = fault_summary(&received);
                let _ = (pc, far, fault);
                if let Some(task) = running.take() {
                    let _ = task.destroy();
                }
                // Reset the service budget so the restart does not leak.
                // SAFETY: only the service owned capabilities under this
                // budget; its task is gone, so the reset is exclusive.
                unsafe {
                    let _ = cnode.revoke(SUB_UNTYPED_SLOT);
                }
                restarts += 1;
                if restarts > MAX_CONSECUTIVE_RESTARTS {
                    fail(&info);
                }
            }
            _ => {}
        }
    }
}

/// Spawn the console service with its endpoints, UART device and budget.
fn spawn_console(console_elf: &[u8]) -> Result<Task, Error> {
    let spawn_info = SpawnInfo {
        magic: SpawnInfo::MAGIC,
        version: SpawnInfo::VERSION,
        control_ep: CONTROL_EP,
        command_ep: COMMAND_EP,
        untyped: CONSOLE_BUDGET,
        rom_start: 0,
        rom_count: 0,
        extra: {
            let mut extra = [0; 8];
            extra[SpawnInfo::CONSOLE_EP] = CONSOLE_EP_CHILD;
            extra[1] = UART_DEV_CHILD;
            extra[2] = CONSOLE_BUDGET;
            extra
        },
    };
    let info_bytes = unsafe {
        core::slice::from_raw_parts(
            &spawn_info as *const SpawnInfo as *const u8,
            core::mem::size_of::<SpawnInfo>(),
        )
    };
    let caps = [
        ChildCap {
            slot: CONTROL_EP,
            source: CONTROL_OBJ,
            rights: RIGHTS_ALL,
            badge: SERVICE_BADGE,
        },
        ChildCap {
            slot: COMMAND_EP,
            source: COMMAND_OBJ,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: CONSOLE_EP_CHILD,
            source: CONSOLE_EP_OWN,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: UART_DEV_CHILD,
            source: UART_DEV_OWN,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: CONSOLE_BUDGET,
            source: SUB_UNTYPED_SLOT,
            rights: RIGHTS_ALL,
            badge: 0,
        },
        ChildCap {
            slot: 6, // INIT_ASID_POOL in the child
            source: INIT_ASID_POOL,
            rights: RIGHTS_ALL,
            badge: 0,
        },
    ];
    unsafe {
        rstiny::elf::spawn_supervised(
            console_elf,
            CHILD_SCRATCH,
            SUB_UNTYPED_SLOT,
            &rstiny::elf::Supervision {
                info: info_bytes,
                fault_ep: CONTROL_EP,
                caps: &caps,
            },
        )
    }
}

/// Map the granted ROM Frame capabilities read-only at `ROM_VA` and return
/// the archive bytes.
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
        if let Err(error) = ensure_table(&cnode, &untyped, va, &mut tables, 1000) {
            rstiny::debug_println!("[init] ensure_table({}) err {:?}", index, error);
            return Err(error);
        }
        if let Err(error) = unsafe {
            Page(CPtr(info.rom_start + index as u64)).map(
                CPtr(INIT_VSPACE),
                va,
                RIGHTS_READ,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )
        } {
            rstiny::debug_println!("[init] rom page map({}) err {:?}", index, error);
            return Err(error);
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
    let slot = first_slot;
    if let Err(error) = untyped.retype(ObjectType::PageTable, 0, cnode.0, slot, 1) {
        rstiny::debug_println!("[init] table retype slot={} err {:?}", slot, error);
        return Err(error);
    }
    if let Err(error) = PageTable(CPtr(slot)).map(CPtr(INIT_VSPACE), address & !0x1F_FFFF) {
        rstiny::debug_println!("[init] table map err {:?}", error);
        return Err(error);
    }
    tables[index] = true;
    Ok(())
}

/// Find a module's contents in the archive bytes. The archive is not
/// null-terminated; names and data are borrowed in place.
fn find_module<'a>(rom: &'a [u8], name: &str) -> Option<&'a [u8]> {
    use rstiny_newc::BootArchive;
    let archive = BootArchive::parse(rom).ok()?;
    archive
        .modules()
        .iter()
        .find(|(module_name, _)| *module_name == name.as_bytes())
        .map(|(_, data)| *data)
}

fn fail(info: &SpawnInfo) -> ! {
    fail_reason(info, 1)
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
