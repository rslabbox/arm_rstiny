//! Supervised spawning: budget, endpoints, devices, IRQ copies, the
//! client console cap and the ELF loader invocation
//! (docs/service-manager.md §5). Also hosts the ROM archive helpers.

use alloc::vec::Vec;
use rstiny::capability::*;
use rstiny::elf::{ChildCap, LOADER_SLOT_BASE, Supervision};
use rstiny::{Error};
use rstiny_protocol::{SpawnInfo};

use crate::state::*;

pub fn spawn_service(services: &mut Vec<ServiceState>, index: usize, console_ep: u64, rom: &[u8]) {
    let Some(elf) = find_module(rom, &services[index].cfg.elf) else {
        services[index].status = Status::Failed;
        return;
    };
    let service = &services[index];
    // Devices the child receives, as (child slot, init-side copy slot) pairs.
    let devices: Vec<(u64, u64)> = service
        .cfg
        .devices
        .iter()
        .enumerate()
        .take(MAX_DEVICES)
        .map(|(k, _)| (CHILD_DEV_BASE + k as u64, service.device_copies[k]))
        .collect();
    // Dependency endpoints in `depends` order: source slots in init's CSpace.
    let deps: Vec<(u64, u64)> = service
        .cfg
        .depends
        .iter()
        .enumerate()
        .take(MAX_DEPS)
        .map(|(j, name)| {
            let source = services
                .iter()
                .position(|s| s.cfg.name == *name)
                .map(|dep_index| services[dep_index].svc_ep_obj)
                .unwrap_or(0);
            (CHILD_DEP_BASE + j as u64, source)
        })
        .collect();
    let spawn_info = SpawnInfo {
        magic: SpawnInfo::MAGIC,
        version: SpawnInfo::VERSION,
        control_ep: CHILD_CONTROL,
        command_ep: 0, // v1: STOP rides the service's main endpoint
        untyped: CHILD_BUDGET,
        rom_start: 0,
        rom_count: 0,
        extra: {
            let mut extra = [0; SpawnInfo::EXTRA_LEN];
            extra[SpawnInfo::CONSOLE_EP] = CHILD_CONSOLE;
            extra[SpawnInfo::DEVICE_SLOT] = CHILD_DEV_BASE;
            extra[SpawnInfo::SELF_EP] = CHILD_SELF_EP;
            extra[SpawnInfo::DEVICE_COUNT] = devices.len() as u64;
            extra[SpawnInfo::DEP_COUNT] = deps.len() as u64;
            for (j, (slot, _)) in deps.iter().enumerate() {
                extra[SpawnInfo::DEP_EP_BASE + j] = *slot;
            }
            extra[SpawnInfo::IRQ_SLOT] = if service.irq_masters[0] != 0 {
                CHILD_IRQ
            } else {
                0
            };
            extra[SpawnInfo::IRQ_SLOT2] = if service.irq_masters[1] != 0 {
                CHILD_IRQ2
            } else {
                0
            };
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
    // pool, own endpoint, then devices and dependency endpoints.
    let (budget_slot, badge) = (service.budget_slot, badge_for(index));
    let mut caps = [ChildCap {
        slot: 0,
        source: 0,
        rights: 0,
        badge: 0,
    }; 16];
    caps[0] = ChildCap {
        slot: CHILD_CONTROL,
        source: CONTROL_OBJ,
        rights: RIGHTS_ALL,
        badge,
    };
    caps[1] = ChildCap {
        slot: CHILD_CONSOLE,
        source: console_ep,
        rights: RIGHTS_ALL,
        badge: 0,
    };
    caps[2] = ChildCap {
        slot: CHILD_BUDGET,
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
    caps[4] = ChildCap {
        slot: CHILD_SELF_EP,
        source: service.svc_ep_obj,
        rights: RIGHTS_ALL,
        badge: 0,
    };
    let mut used = 5;
    for &(slot, source) in devices.iter().chain(deps.iter()) {
        if source == 0 || used == caps.len() {
            services[index].status = Status::Failed;
            return;
        }
        caps[used] = ChildCap {
            slot,
            source,
            rights: RIGHTS_ALL,
            badge: 0,
        };
        used += 1;
    }
    // The device IRQ handlers, for every device this service drives an
    // interrupt for (docs/irq.md §8): the child binds its own Notifications.
    for (k, &master) in service.irq_masters.iter().enumerate() {
        if master == 0 {
            continue;
        }
        if used == caps.len() {
            services[index].status = Status::Failed;
            return;
        }
        caps[used] = ChildCap {
            slot: if k == 0 { CHILD_IRQ } else { CHILD_IRQ2 },
            source: service.irq_copies[k],
            rights: RIGHTS_ALL,
            badge: 0,
        };
        used += 1;
    }

    let spawned = unsafe {
        // The service's allocations carve its own per-service sub-region, so
        // the teardown revoke (Task::destroy) is scoped to this service only.
        rstiny::elf::spawn_supervised(
            elf,
            CHILD_SCRATCH,
            service.budget_slot,
            &rstiny::elf::Supervision {
                info: info_bytes,
                argv: &[],
                fault_ep: CHILD_CONTROL,
                caps: &caps[..used],
                slot_base: rstiny::elf::LOADER_SLOT_BASE
                    + index as u64 * rstiny::elf::LOADER_SLOT_STRIDE,
            },
        )
    };
    match spawned {
        Ok(task) => {
            services[index].task = Some(task);
            services[index].status = Status::Starting;
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
pub fn map_rom(info: &SpawnInfo) -> Result<&'static [u8], Error> {
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

pub fn ensure_table(
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
pub fn find_module<'a>(rom: &'a [u8], name: &str) -> Option<&'a [u8]> {
    use rstiny_newc::BootArchive;
    let archive = BootArchive::parse(rom).ok()?;
    archive
        .modules()
        .iter()
        .find(|(module_name, _)| *module_name == name.as_bytes())
        .map(|(_, data)| *data)
}

