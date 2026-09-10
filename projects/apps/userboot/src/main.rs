#![no_std]
#![no_main]
//! userboot: the root task. Maps the boot module archive, spawns init (the
//! service manager) with a supervised control endpoint and restarts it if it
//! fails (docs/service-manager.md, level-1 supervision).
use kernel_abi::BootModules;
use rstiny::capability::*;
use rstiny::elf::ChildCap;
use rstiny::ipc;
use rstiny_newc::BootArchive;
use rstiny_protocol::{SpawnInfo, control};
use rstiny_runtime::{BootInfo, entry};

const CONTROL_EP: u64 = 130; // root's supervision endpoint object for init
const INIT_BUDGET_OBJ: u64 = 160; // init's Untyped budget carved from our pool
const UART_DEV_COPY: u64 = 161; // UART device Untyped copy for init
const INIT_CONTROL_SLOT: u64 = 140; // init's control endpoint slot
const INIT_BUDGET_SLOT: u64 = 32; // init's budget slot in its own CSpace
const INIT_DEV_SLOT: u64 = 161; // init's first device Untyped slot in its CSpace
const INIT_ASID_SLOT: u64 = 6; // init's ASID pool slot (the standard slot)
const INIT_ROM_FIRST: u64 = 200; // init's ROM Frame caps (clear of 161)
const ROM_GRANT_MAX: usize = 512; // ROM pages granted to init (covers the whole archive)

const INIT_ELF: &str = "init.elf";
const SERVICE_BADGE: u64 = 1;
const MAX_INIT_RESTARTS: u32 = 5;
/// Device Untyped copies handed to init (there is exactly one: the UART).
fn boot_test() -> bool {
    option_env!("BOOT_TEST").is_some_and(|value| value == "1")
}

#[entry(stack_size = 64 * 1024)]
fn main(info: &mut BootInfo) -> ! {
    let modules = info.boot_modules();
    let free_slot = info.untyped_start() + info.untyped().len() as u64;
    let rom = match map_rom(info, modules, free_slot) {
        Ok(rom) => rom,
        Err(error) => {
            rstiny::debug_println!("[userboot] map_rom failed: {:?}", error);
            root_failed()
        }
    };
    let Some(init_elf) = find_module(&rom, INIT_ELF) else {
        root_failed();
    };
    let scratch = info.first_free_address();
    // Device Untyped copies for init: the boot partition has exactly one
    // device region (the UART) today.
    let device_count = info.untyped().iter().filter(|d| d.is_device != 0).count();
    let first_device = info
        .untyped()
        .iter()
        .position(|d| d.is_device != 0)
        .map(|index| info.untyped_start() + index as u64)
        .unwrap_or(0);
    let Some(_uart_slot) = info
        .untyped()
        .iter()
        .position(|descriptor| descriptor.is_device != 0)
        .map(|index| info.untyped_start() + index as u64)
    else {
        root_failed();
    };
    let cnode = CNode(CPtr(INIT_CNODE));
    let allocator = Untyped(CPtr(INIT_UNTYPED));
    if cnode
        .retype_endpoint(CPtr(INIT_UNTYPED), CONTROL_EP)
        .is_err()
    {
        rstiny::debug_println!("[userboot] control ep retype failed");
        root_failed();
    }
    // Grant the UART device Untyped itself so the console driver can retype
    // its MMIO frame (the device region is passed through, never retyped).
    if cnode
        .copy(UART_DEV_COPY, CPtr(INIT_CNODE), first_device, RIGHTS_ALL)
        .is_err()
    {
        rstiny::debug_println!("[userboot] uart device copy failed");
        root_failed();
    }
    // init's budget: carve a sub-Untyped after the small allocations above so
    // the rest of the region stays available to the root task.
    let largest_bits = info.untyped().first().map(|d| d.size_bits).unwrap_or(0);
    let mut budget_bits = 23u64.min(largest_bits.saturating_sub(1));
    while budget_bits >= 12 {
        if allocator
            .retype(
                ObjectType::Untyped,
                budget_bits,
                cnode.0,
                INIT_BUDGET_OBJ,
                1,
            )
            .is_ok()
        {
            break;
        }
        budget_bits -= 1;
    }
    if budget_bits < 12 {
        rstiny::debug_println!("[userboot] budget retype failed");
        root_failed();
    }

    let mut restarts: u32 = 0;
    loop {
        let spawn_info = SpawnInfo {
            magic: SpawnInfo::MAGIC,
            version: SpawnInfo::VERSION,
            control_ep: INIT_CONTROL_SLOT,
            command_ep: 0,
            untyped: INIT_BUDGET_SLOT,
            rom_start: INIT_ROM_FIRST,
            rom_count: rom.frame_count.min(ROM_GRANT_MAX as u64),
            extra: {
                let mut extra = [0; 8];
                extra[1] = INIT_DEV_SLOT;
                extra[3] = device_count as u64;
                extra[4] = u64::from(boot_test());
                extra
            },
        };
        let info_bytes = unsafe {
            core::slice::from_raw_parts(
                &spawn_info as *const SpawnInfo as *const u8,
                core::mem::size_of::<SpawnInfo>(),
            )
        };
        // Fixed-capability prefix plus the ROM window, on the stack.
        let mut caps = [ChildCap {
            slot: 0,
            source: 0,
            rights: 0,
            badge: 0,
        }; 4 + ROM_GRANT_MAX];
        caps[0] = ChildCap {
            slot: INIT_CONTROL_SLOT,
            source: CONTROL_EP,
            rights: RIGHTS_ALL,
            badge: SERVICE_BADGE,
        };
        caps[1] = ChildCap {
            slot: INIT_BUDGET_SLOT,
            source: INIT_BUDGET_OBJ,
            rights: RIGHTS_ALL,
            badge: 0,
        };
        caps[2] = ChildCap {
            slot: INIT_DEV_SLOT,
            source: UART_DEV_COPY,
            rights: RIGHTS_ALL,
            badge: 0,
        };
        caps[3] = ChildCap {
            slot: INIT_ASID_SLOT,
            source: INIT_ASID_POOL,
            rights: RIGHTS_ALL,
            badge: 0,
        };
        let granted = (modules.frame_count as usize).min(ROM_GRANT_MAX);
        for (index, cap) in caps[4..4 + granted].iter_mut().enumerate() {
            *cap = ChildCap {
                slot: INIT_ROM_FIRST + index as u64,
                source: kernel_abi::INIT_BOOT_MODULES + index as u64,
                rights: RIGHTS_READ,
                badge: 0,
            };
        }

        match unsafe {
            rstiny::elf::spawn_supervised(
                init_elf,
                scratch,
                INIT_BUDGET_OBJ,
                &rstiny::elf::Supervision {
                    info: info_bytes,
                    fault_ep: INIT_CONTROL_SLOT,
                    caps: &caps[..4 + granted],
                },
            )
        } {
            Ok(task) => {
                // Level-1 supervision: only init's failures arrive here.
                loop {
                    let received = match ipc::recv(CONTROL_EP) {
                        Ok(received) => received,
                        Err(error) => {
                            rstiny::debug_println!("[userboot] recv err: {:?}", error);
                            continue;
                        }
                    };
                    match received.label {
                        control::READY if received.badge == SERVICE_BADGE => {
                            restarts = 0;
                            // READY arrived as a Call: answer so init resumes.
                            let _ = ipc::reply(0, &[]);
                        }
                        control::REPORT if received.badge == SERVICE_BADGE => {
                            let _ = ipc::reply(0, &[]);
                        }
                        // EXIT or a kernel fault: no reply — the sender is
                        // torn down below.
                        control::EXIT | 0..=4 => break,
                        _ => {}
                    }
                }
                let _ = task.destroy();
            }
            Err(error) => {
                rstiny::debug_println!("[userboot] spawn failed: {:?}", error);
            }
        }
        restarts += 1;
        if restarts > MAX_INIT_RESTARTS {
            root_failed();
        }
        // Reset init's budget watermark so the restart does not leak.
        // SAFETY: init's derivation subtree was already revoked by destroy().
        unsafe {
            let _ = cnode.revoke(INIT_BUDGET_OBJ);
        }
    }
}

struct Rom<'a> {
    bytes: &'a [u8],
    frame_count: u64,
}

/// Map the boot module archive read-only and verify it parses.
fn map_rom(
    info: &BootInfo,
    modules: BootModules,
    free_slot: u64,
) -> Result<Rom<'static>, rstiny::Error> {
    if modules.paddr == 0 || modules.frame_count == 0 {
        return Err(rstiny::Error::InvalidArgument);
    }
    let cnode = CNode(CPtr(INIT_CNODE));
    let untyped = Untyped(CPtr(INIT_UNTYPED));
    let scratch = info.first_free_address();
    let rom_va = scratch.next_multiple_of(0x20_0000) + 0x20_0000;
    let mut tables = [false; 64];
    let mut slot_cursor = free_slot;
    for index in 0..modules.frame_count as usize {
        let va = rom_va + index * 4096;
        let table_index = va >> 21;
        if table_index >= 64 {
            return Err(rstiny::Error::InvalidArgument);
        }
        if !tables[table_index] {
            let slot = slot_cursor;
            slot_cursor += 1;
            untyped.retype(ObjectType::PageTable, 0, cnode.0, slot, 1)?;
            PageTable(CPtr(slot)).map(CPtr(INIT_VSPACE), va & !0x1F_FFFF)?;
            tables[table_index] = true;
        }
        unsafe {
            Page(CPtr(kernel_abi::INIT_BOOT_MODULES + index as u64)).map(
                CPtr(INIT_VSPACE),
                va,
                RIGHTS_READ,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )?;
        }
    }
    // SAFETY: the ROM window is mapped read-only for this task's lifetime.
    let bytes = unsafe {
        core::slice::from_raw_parts(rom_va as *const u8, modules.frame_count as usize * 4096)
    };
    BootArchive::parse(bytes).map_err(|_| rstiny::Error::InvalidArgument)?;
    Ok(Rom {
        bytes,
        frame_count: modules.frame_count,
    })
}

fn find_module<'a>(rom: &Rom<'a>, name: &str) -> Option<&'a [u8]> {
    let archive = BootArchive::parse(rom.bytes).ok()?;
    archive
        .modules()
        .iter()
        .find(|(module_name, _)| *module_name == name.as_bytes())
        .map(|(_, data)| *data)
}

fn root_failed() -> ! {
    // No supervisor above the root task: park instead of spinning hot.
    loop {
        core::hint::spin_loop();
    }
}
