//! Adopt elfloader's loaded root image; the kernel neither embeds nor loads ELF files.
use crate::memory::address::phys_to_virt;
use crate::{arch::kernel::boot, memory};
use alloc::vec::Vec;
use kernel_abi::*;

/// Partition free RAM into aligned power-of-two Untyped regions, excluding
/// every reserved physical extent. Device MMIO is appended explicitly.
fn boot_regions(loaded: boot::BootInfo) -> Vec<(usize, u8, bool)> {
    use crate::config::{FIRMWARE_END, LOADER_END, LOADER_START, PAGE_SIZE, RAM_END, RAM_START};
    let kernel_end = memory::address::kernel_image()
        .expect("kernel image")
        .physical_end()
        .as_usize();
    let modules_end = loaded.modules + loaded.modules_size;
    let mut reserved = [
        (RAM_START, FIRMWARE_END),
        (loaded.kernel_physical, kernel_end),
        (loaded.dtb, loaded.dtb + loaded.dtb_size),
        (loaded.image_start, loaded.image_end + PAGE_SIZE),
        (LOADER_START, LOADER_END),
        (loaded.modules, modules_end),
    ];
    if loaded.modules == 0 {
        reserved[5] = (0, 0);
    }
    let mut free: Vec<(usize, usize)> = alloc::vec![(RAM_START, RAM_END)];
    for &(start, end) in &reserved {
        let mut next = Vec::new();
        for &(begin, limit) in &free {
            if end <= begin || start >= limit {
                next.push((begin, limit));
            } else {
                if begin < start {
                    next.push((begin, start));
                }
                if end < limit {
                    next.push((end, limit));
                }
            }
        }
        free = next;
    }
    let mut regions = Vec::new();
    let mut devices = Vec::new();
    for (start, end) in free {
        crate::object::partition(start, end, |physical, size_bits| {
            regions.push((physical, size_bits, false));
        });
    }
    // Device MMIO: the UART and the VirtIO MMIO window are available to user
    // drivers; GIC and timer stay with the kernel and are never published.
    // Devices keep ascending physical order regardless of size so supervisor
    // device tables (docs/disk-driver.md section 5) are stable.
    devices.push((crate::config::UART_BASE, 12, true));
    devices.push((
        crate::config::VIRTIO_MMIO_BASE,
        crate::config::VIRTIO_MMIO_SIZE_LOG2,
        true,
    ));
    devices.sort_by_key(|&(physical, _, _)| physical);
    // Put the largest ordinary regions first so the well-known first Untyped
    // capability (`INIT_UNTYPED`) can back a full ELF load.
    regions.sort_by(|a, b| b.1.cmp(&a.1));
    regions.extend(devices);
    regions
}

#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn start_root() -> ! {
    let loaded = boot::information();
    let image =
        loaded.image_start - loaded.phys_virt_offset..loaded.image_end - loaded.phys_virt_offset;
    let layout =
        InitialTaskLayout::new(image.start as u64..image.end as u64, loaded.dtb_size as u64)
            .expect("validated root layout");
    memory::prepare_boot(loaded.image_start, loaded.image_end);
    memory::frame::prepare_modules(loaded.modules, loaded.modules + loaded.modules_size);
    let vspace = crate::object::boot_vspace().expect("root VSpace");
    // seL4 elfloader keeps {u32 phnum, u32 phsize, program headers} in the
    // page immediately following the loaded region. Use that existing metadata
    // solely to retain this kernel's segment permissions and unmapped holes.
    // SAFETY: the boot contract validated and mapped this reserved physical page.
    let headers = unsafe {
        core::slice::from_raw_parts(
            phys_to_virt(memory_addr::PhysAddr::from_usize(loaded.image_end))
                .expect("direct-map address")
                .as_usize() as *const u8,
            PAGE_SIZE as usize,
        )
    };
    let count = u32::from_le_bytes(headers[..4].try_into().unwrap()) as usize;
    let size = u32::from_le_bytes(headers[4..8].try_into().unwrap()) as usize;
    assert!(count > 0 && count <= 32 && size == 56 && 8 + count * size <= headers.len());
    let mut valid_entry = false;
    for index in 0..count {
        let record = &headers[8 + index * size..8 + (index + 1) * size];
        let kind = u32::from_le_bytes(record[..4].try_into().unwrap());
        if kind != 1 {
            continue;
        }
        let flags = u32::from_le_bytes(record[4..8].try_into().unwrap());
        let word =
            |offset| u64::from_le_bytes(record[offset..offset + 8].try_into().unwrap()) as usize;
        let va = word(16);
        let file_size = word(32);
        let memory_size = word(40);
        if memory_size == 0 {
            continue;
        }
        let end = va.checked_add(memory_size).expect("user image overflow");
        assert!(va >= image.start && end <= image.end);
        assert!(va.is_multiple_of(PAGE_SIZE as usize) && file_size <= memory_size);
        let rights = match flags {
            4 => 1,
            5 => 5,
            6 => 3,
            _ => panic!("unsupported root permissions"),
        };
        valid_entry |= rights == 5 && (va..va + file_size).contains(&loaded.entry);
        let physical = va
            .checked_add(loaded.phys_virt_offset)
            .expect("user physical overflow");
        crate::object::boot_map_loaded(
            vspace,
            va,
            physical,
            memory_size.next_multiple_of(PAGE_SIZE as usize),
            rights,
        )
        .expect("root loaded mapping");
    }
    assert!(
        valid_entry,
        "root entry outside initialized executable segment"
    );
    memory::finish_boot();
    // Partition free RAM before any user object can be created. The resulting
    // Untyped objects are published as a contiguous capability range.
    let regions = boot_regions(loaded);
    assert!(
        regions.len() <= MAX_UNTYPED_REGIONS,
        "too many Untyped regions"
    );
    crate::object::boot_untyped(&regions).expect("boot Untyped regions");
    let modules_end = loaded.modules + loaded.modules_size;
    crate::object::boot_modules(loaded.modules..modules_end);
    let untyped_start = INIT_UNTYPED;
    // Match seL4: metadata follows the actual page-rounded ELF image end.
    crate::object::boot_map(
        vspace,
        layout.ipc_buffer as usize,
        PAGE_SIZE as usize,
        3,
        true,
    )
    .expect("root IPC buffer");
    crate::object::boot_map(
        vspace,
        layout.boot_info as usize,
        PAGE_SIZE as usize,
        1,
        true,
    )
    .expect("root BootInfo");
    // Forward the opaque DTB and the Untyped descriptor list as extended
    // BootInfo, without parsing the DTB contents.
    let header_size = core::mem::size_of::<BootInfoHeader>();
    let fdt_size = header_size + loaded.dtb_size;
    crate::object::boot_map(
        vspace,
        layout.extra as usize,
        (layout.extra_size as usize).next_multiple_of(memory::PAGE_SIZE),
        1,
        true,
    )
    .expect("root extra BootInfo");
    let fdt_header = BootInfoHeader {
        id: BOOTINFO_HEADER_FDT,
        len: fdt_size as u64,
    };
    // SAFETY: two initialized u64 fields; DTB extent was checked during boot.
    let fdt_header_bytes = unsafe {
        core::slice::from_raw_parts(
            (&fdt_header as *const BootInfoHeader).cast::<u8>(),
            header_size,
        )
    };
    let dtb = unsafe {
        core::slice::from_raw_parts(
            phys_to_virt(memory_addr::PhysAddr::from_usize(loaded.dtb))
                .expect("direct-map address")
                .as_usize() as *const u8,
            loaded.dtb_size,
        )
    };
    crate::object::boot_write(vspace, layout.extra as usize, fdt_header_bytes)
        .expect("extra BootInfo FDT header");
    crate::object::boot_write(vspace, layout.extra as usize + header_size, dtb)
        .expect("extra BootInfo DTB");
    let untyped_offset = (header_size + loaded.dtb_size).next_multiple_of(8);
    let untyped_record_size = header_size + regions.len() * core::mem::size_of::<UntypedDesc>();
    let untyped_header = BootInfoHeader {
        id: BOOTINFO_HEADER_UNTYPED,
        len: untyped_record_size as u64,
    };
    let untyped_header_bytes = unsafe {
        core::slice::from_raw_parts(
            (&untyped_header as *const BootInfoHeader).cast::<u8>(),
            header_size,
        )
    };
    crate::object::boot_write(
        vspace,
        layout.extra as usize + untyped_offset,
        untyped_header_bytes,
    )
    .expect("extra BootInfo Untyped header");
    for (index, &(physical, size_bits, is_device)) in regions.iter().enumerate() {
        let desc = UntypedDesc {
            paddr: physical as u64,
            size_bits: size_bits as u64,
            is_device: is_device as u64,
            reserved: 0,
        };
        // SAFETY: a fully initialized repr(C) record with no padding.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&desc as *const UntypedDesc).cast::<u8>(),
                core::mem::size_of::<UntypedDesc>(),
            )
        };
        crate::object::boot_write(
            vspace,
            layout.extra as usize
                + untyped_offset
                + header_size
                + index * core::mem::size_of::<UntypedDesc>(),
            bytes,
        )
        .expect("extra BootInfo Untyped descriptor");
    }
    // The boot-module record: physical extent plus the Frame cap range the
    // root task received in its initial CNode.
    let modules_header = BootInfoHeader {
        id: BOOTINFO_HEADER_BOOT_MODULES,
        len: header_size as u64 + core::mem::size_of::<BootModules>() as u64,
    };
    let modules_offset = untyped_offset + untyped_record_size.next_multiple_of(8);
    let modules_record = BootModules {
        paddr: loaded.modules as u64,
        size: (loaded.modules_size) as u64,
        frame_start: INIT_BOOT_MODULES,
        frame_count: (loaded.modules_size / (PAGE_SIZE as usize)) as u64,
        reserved: [0; 4],
    };
    // SAFETY: fully initialized repr(C) records with no padding.
    let (modules_header_bytes, modules_record_bytes) = unsafe {
        (
            core::slice::from_raw_parts(
                (&modules_header as *const BootInfoHeader).cast::<u8>(),
                header_size,
            ),
            core::slice::from_raw_parts(
                (&modules_record as *const BootModules).cast::<u8>(),
                core::mem::size_of::<BootModules>(),
            ),
        )
    };
    crate::object::boot_write(
        vspace,
        layout.extra as usize + modules_offset,
        modules_header_bytes,
    )
    .expect("extra BootInfo modules header");
    crate::object::boot_write(
        vspace,
        layout.extra as usize + modules_offset + header_size,
        modules_record_bytes,
    )
    .expect("extra BootInfo modules record");
    let info = BootInfo {
        magic: BOOTINFO_MAGIC,
        version: ABI_VERSION,
        size: core::mem::size_of::<BootInfo>() as u64,
        page_size: PAGE_SIZE,
        features: if log::max_level() != log::LevelFilter::Off {
            FEATURE_DEBUG_CONSOLE
        } else {
            0
        },
        ipc_buffer: layout.ipc_buffer,
        extra: layout.extra,
        extra_size: layout.extra_size,
        untyped_start,
        untyped_count: regions.len() as u64,
        reserved: [0; 6],
    };
    // SAFETY: BootInfo contains only initialized u64 fields, with no padding.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&info as *const BootInfo).cast::<u8>(),
            core::mem::size_of::<BootInfo>(),
        )
    };
    crate::object::boot_write(vspace, layout.boot_info as usize, bytes)
        .expect("BootInfo initialization");
    log::info!(
        "Starting fatboot: entry={:#x}, BootInfo={:#x}, Untyped={} regions, EL0",
        loaded.entry,
        layout.boot_info,
        regions.len()
    );
    let root = crate::object::vspace_root(vspace).expect("root page table");
    crate::task::start(
        vspace,
        root,
        loaded.entry as u64,
        layout.boot_info,
        untyped_start,
        crate::api::dispatch,
    )
}
