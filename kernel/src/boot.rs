//! Adopt elfloader's loaded root image; the kernel neither embeds nor loads ELF files.
use crate::{
    arch::boot,
    config::phys_to_virt,
    memory::{self, AddressSpace},
};
use kernel_abi::*;

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
    let mut space = AddressSpace::new().expect("root page tables");
    // seL4 elfloader keeps {u32 phnum, u32 phsize, program headers} in the
    // page immediately following the loaded region. Use that existing metadata
    // solely to retain this kernel's segment permissions and unmapped holes.
    // SAFETY: the boot contract validated and mapped this reserved physical page.
    let headers = unsafe {
        core::slice::from_raw_parts(
            phys_to_virt(loaded.image_end) as *const u8,
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
        space
            .map_loaded(
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
    // Match seL4: metadata follows the actual page-rounded ELF image end.
    space
        .map(layout.ipc_buffer as usize, PAGE_SIZE as usize, 3, true)
        .expect("root IPC buffer");
    space
        .map(layout.boot_info as usize, PAGE_SIZE as usize, 1, true)
        .expect("root BootInfo");
    // Forward the opaque DTB as extended BootInfo, without parsing its contents.
    let header_size = core::mem::size_of::<BootInfoHeader>();
    let extra_size = header_size + loaded.dtb_size;
    space
        .map(
            layout.extra as usize,
            extra_size.next_multiple_of(PAGE_SIZE as usize),
            1,
            true,
        )
        .expect("root extra BootInfo");
    let header = BootInfoHeader {
        id: BOOTINFO_HEADER_FDT,
        len: extra_size as u64,
    };
    // SAFETY: two initialized u64 fields; DTB extent was checked during boot.
    let header_bytes = unsafe {
        core::slice::from_raw_parts((&header as *const BootInfoHeader).cast::<u8>(), header_size)
    };
    let dtb = unsafe {
        core::slice::from_raw_parts(phys_to_virt(loaded.dtb) as *const u8, loaded.dtb_size)
    };
    space
        .initialize(layout.extra as usize, header_bytes)
        .expect("extra BootInfo header");
    space
        .initialize(layout.extra as usize + header_size, dtb)
        .expect("extra BootInfo DTB");
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
        extra_size: extra_size as u64,
    };
    // SAFETY: BootInfo contains only initialized u64 fields, with no padding.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&info as *const BootInfo).cast::<u8>(),
            core::mem::size_of::<BootInfo>(),
        )
    };
    space
        .initialize(layout.boot_info as usize, bytes)
        .expect("BootInfo initialization");
    log::info!(
        "Starting fatboot: entry={:#x}, BootInfo={:#x}, EL0",
        loaded.entry,
        layout.boot_info
    );
    crate::task::start(
        space,
        loaded.entry as u64,
        layout.boot_info,
        crate::syscall::dispatch,
    )
}
