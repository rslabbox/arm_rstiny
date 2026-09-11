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
const DEVICE_COPY_BASE: u64 = 161; // device Untyped copies, +k per device region
const IRQ_MASTER_BASE: u64 = 170; // root-side IRQHandler masters, +i per user line
const MAX_DEVICES: usize = 4;
const MAX_IRQ_LINES: usize = 64; // bound matching the BootInfo record reservation
const INIT_CONTROL_SLOT: u64 = 140; // init's control endpoint slot
const INIT_BUDGET_SLOT: u64 = 32; // init's budget slot in its own CSpace
const INIT_DEV_SLOT: u64 = 161; // init's first device Untyped slot in its CSpace
const INIT_IRQ_SLOT: u64 = 800; // init's first IRQHandler slot (clear of the ROM window)
const INIT_ASID_SLOT: u64 = 6; // init's ASID pool slot (the standard slot)
const INIT_ROM_FIRST: u64 = 200; // init's ROM Frame caps (clear of 161)
const ROM_GRANT_MAX: usize = 512; // ROM pages granted to init (covers the whole archive)
/// init's budget: console + block + fs + appmgr service budgets (8 MiB) plus
/// init's own objects. Achieving it is a boot precondition, not best effort.
const INIT_BUDGET_BITS: u64 = 24;

const INIT_ELF: &str = "init.elf";
const SERVICE_BADGE: u64 = 1;
const MAX_INIT_RESTARTS: u32 = 5;

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
    // Device Untyped copies for init, one per device region in ascending
    // physical order (boot.rs publishes UART before the VirtIO window).
    let mut devices = [0u64; MAX_DEVICES]; // source cap slots in our CSpace
    let mut device_count = 0usize;
    for (index, descriptor) in info.untyped().iter().enumerate() {
        if descriptor.is_device == 0 {
            continue;
        }
        if device_count == MAX_DEVICES {
            rstiny::debug_println!("[userboot] too many device regions");
            root_failed();
        }
        devices[device_count] = info.untyped_start() + index as u64;
        device_count += 1;
    }
    let cnode = CNode(CPtr(INIT_CNODE));
    if cnode
        .retype_endpoint(CPtr(INIT_UNTYPED), CONTROL_EP)
        .is_err()
    {
        rstiny::debug_println!("[userboot] control ep retype failed");
        root_failed();
    }
    // Grant every device Untyped so drivers can retype their MMIO frames
    // (device regions are passed through, never retyped or split).
    for (index, &slot) in devices[..device_count].iter().enumerate() {
        if cnode
            .copy(
                DEVICE_COPY_BASE + index as u64,
                CPtr(INIT_CNODE),
                slot,
                RIGHTS_ALL,
            )
            .is_err()
        {
            rstiny::debug_println!("[userboot] device copy failed");
            root_failed();
        }
    }
    // Authorize every platform-table line as a root-side master (docs/irq.md
    // §8). The kernel publishes the table in BootInfo — authorization policy
    // is data, not probing. Table order: VirtIO slot lines ascending, other
    // device lines after; QEMU attaches devices to the window's end, so
    // virtio device N uses kind-0 entry count-1-N (docs/irq.md §7).
    let lines = info.irq_lines();
    if lines.len() > MAX_IRQ_LINES {
        rstiny::debug_println!("[userboot] too many user IRQ lines");
        root_failed();
    }
    for (index, line) in lines.iter().enumerate() {
        if IrqControl(CPtr(INIT_IRQ_CONTROL))
            .get(line.intid, cnode.0, IRQ_MASTER_BASE + index as u64)
            .is_err()
        {
            rstiny::debug_println!("[userboot] IRQ master {} authorize failed", line.intid);
            root_failed();
        }
    }
    let irq_count = lines.len();
    let virtio_lines = lines
        .iter()
        .filter(|line| line.kind == kernel_abi::IRQ_KIND_VIRTIO_SLOT)
        .count();
    if virtio_lines == 0 {
        rstiny::debug_println!("[userboot] platform exposes no VirtIO IRQ lines");
        root_failed();
    }
    // init's budget: carve a whole sub-Untyped from a region big enough that
    // root's own allocations (ROM tables, endpoints) are untouched. The boot
    // partition yields several max-sized regions; pick one other than the
    // first, which backs root's own allocations.
    let budget_source = info
        .untyped()
        .iter()
        .enumerate()
        .find(|(index, descriptor)| {
            descriptor.is_device == 0
                && descriptor.size_bits >= INIT_BUDGET_BITS
                && info.untyped_start() + *index as u64 != INIT_UNTYPED
        })
        .map(|(index, _)| info.untyped_start() + index as u64);
    let Some(source) = budget_source else {
        rstiny::debug_println!("[userboot] init budget ({INIT_BUDGET_BITS} bits) unavailable");
        root_failed();
    };
    if Untyped(CPtr(source))
        .retype(
            ObjectType::Untyped,
            INIT_BUDGET_BITS,
            cnode.0,
            INIT_BUDGET_OBJ,
            1,
        )
        .is_err()
    {
        rstiny::debug_println!("[userboot] init budget retype failed");
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
                let mut extra = [0; SpawnInfo::EXTRA_LEN];
                extra[1] = INIT_DEV_SLOT;
                extra[3] = device_count as u64;
                extra[4] = u64::from(boot_test());
                // Restart generation: test drills only run in the first
                // incarnation (docs/thread-group.md §7).
                extra[5] = u64::from(restarts);
                // IRQHandler masters: first init-side slot, then the count of
                // VirtIO slot lines among them (docs/irq.md §8). Virtio device
                // N maps to entry count-1-N.
                extra[SpawnInfo::IRQ_SLOT] = INIT_IRQ_SLOT;
                extra[SpawnInfo::IRQ_SLOT + 1] = virtio_lines as u64;
                extra
            },
        };
        let info_bytes = unsafe {
            core::slice::from_raw_parts(
                &spawn_info as *const SpawnInfo as *const u8,
                core::mem::size_of::<SpawnInfo>(),
            )
        };
        // Fixed-capability prefix, device and IRQ grants, then the ROM window.
        let mut caps = [ChildCap {
            slot: 0,
            source: 0,
            rights: 0,
            badge: 0,
        }; 4 + MAX_DEVICES + MAX_IRQ_LINES + ROM_GRANT_MAX];
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
            slot: INIT_ASID_SLOT,
            source: INIT_ASID_POOL,
            rights: RIGHTS_ALL,
            badge: 0,
        };
        for (index, cap) in caps[3..3 + device_count].iter_mut().enumerate() {
            *cap = ChildCap {
                slot: DEVICE_COPY_BASE + index as u64,
                source: DEVICE_COPY_BASE + index as u64,
                rights: RIGHTS_ALL,
                badge: 0,
            };
        }
        for (index, cap) in caps[3 + device_count..3 + device_count + irq_count]
            .iter_mut()
            .enumerate()
        {
            *cap = ChildCap {
                slot: INIT_IRQ_SLOT + index as u64,
                source: IRQ_MASTER_BASE + index as u64,
                rights: RIGHTS_ALL,
                badge: 0,
            };
        }
        let granted = (modules.frame_count as usize).min(ROM_GRANT_MAX);
        let fixed = 3 + device_count + irq_count;
        for (index, cap) in caps[fixed..fixed + granted].iter_mut().enumerate() {
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
                    caps: &caps[..fixed + granted],
                    slot_base: rstiny::elf::LOADER_SLOT_BASE,
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
            // Device regions are shared with userboot's own copies and are not
            // covered by init's budget revoke: without this a restarted driver
            // cannot re-carve its MMIO frames (docs/disk-driver.md §6.4).
            // SAFETY: init's device derivation subtrees are already dead.
            for index in 0..device_count {
                let _ = cnode.revoke(DEVICE_COPY_BASE + index as u64);
            }
        }
        // A crashed init can leave a service's notification bound to a line.
        // Clear the masters so every line starts unbound and quiesced
        // (docs/irq.md §12.3); the next incarnation re-binds on spawn.
        for index in 0..irq_count {
            let _ = IrqHandler(CPtr(IRQ_MASTER_BASE + index as u64)).clear();
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
