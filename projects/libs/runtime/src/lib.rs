#![no_std]
//! Root and ordinary task entrypoints, boot contract and default panic policy.
use kernel_abi as abi;

/// Validated boot data and exclusive access to the initial IPC buffer.
/// Constructed once by the runtime; it cannot be cloned or built by applications.
pub struct BootInfo {
    raw: &'static abi::BootInfo,
    dtb: &'static [u8],
    untyped: &'static [abi::UntypedDesc],
    modules: abi::BootModules,
    irqs: &'static [abi::IrqDesc],
}

impl BootInfo {
    /// Read-only DTB forwarded by the kernel. The application chooses its parser.
    pub fn device_tree(&self) -> &'static [u8] {
        self.dtb
    }

    /// Boot-partitioned physical Untyped regions, in CSpace slot order from
    /// [`BootInfo::untyped_start`].
    pub fn untyped(&self) -> &'static [abi::UntypedDesc] {
        self.untyped
    }

    /// First CSpace slot holding an initial Untyped capability.
    pub fn untyped_start(&self) -> u64 {
        self.raw.untyped_start
    }

    /// The boot-module archive: physical extent and the contiguous read-only
    /// Frame capabilities in this task's CSpace (`frame_start..frame_count`).
    pub fn boot_modules(&self) -> abi::BootModules {
        self.modules
    }

    /// The platform's user-authorizable IRQ lines (docs/irq.md §3.1), in
    /// generated order: VirtIO MMIO slot lines ascending first, other device
    /// lines after. `IRQControl_Get` accepts exactly these.
    pub fn irq_lines(&self) -> &'static [abi::IrqDesc] {
        self.irqs
    }

    /// Capability slot of the largest ordinary Untyped region. A loader that
    /// needs many pages should carve them from one region, not the smallest.
    pub fn largest_untyped(&self) -> Option<u64> {
        self.untyped
            .iter()
            .enumerate()
            .filter(|(_, descriptor)| descriptor.is_device == 0)
            .max_by_key(|(_, descriptor)| descriptor.size_bits)
            .map(|(index, _)| self.raw.untyped_start + index as u64)
    }

    pub fn address(&self) -> usize {
        self.raw as *const _ as usize
    }

    pub fn debug_console_available(&self) -> bool {
        self.raw.features & abi::FEATURE_DEBUG_CONSOLE != 0
    }

    /// First page after the initial image and kernel-supplied metadata.
    /// The root task chooses how to allocate this initially unmapped range.
    pub fn first_free_address(&self) -> usize {
        (self.raw.extra + self.raw.extra_size).next_multiple_of(abi::PAGE_SIZE) as usize
    }

    /// Address of the IPC buffer privately used by the syscall library.
    pub fn ipc_buffer_address(&self) -> usize {
        self.raw.ipc_buffer as usize
    }
}

/// Kernel-to-runtime boundary, called only by the generated entrypoint.
///
/// # Safety
/// Must be invoked exactly once, with the kernel-provided immutable BootInfo
/// mapping and exclusive, zero-initialized IPC mapping valid for task lifetime.
#[doc(hidden)]
#[inline(never)]
#[unsafe(export_name = "__rstiny_root_start")]
pub unsafe fn start(pointer: *const (), main: fn(&mut BootInfo) -> !) -> ! {
    let address = pointer as u64;
    assert!(address >= 2 * abi::PAGE_SIZE && address.is_multiple_of(abi::PAGE_SIZE));
    assert!(
        address
            .checked_add(abi::PAGE_SIZE)
            .is_some_and(|end| end <= abi::USER_ADDRESS_LIMIT)
    );
    // SAFETY: The entry contract supplies a pinned, readable BootInfo page.
    let raw = unsafe { &*pointer.cast::<abi::BootInfo>() };
    assert_eq!(raw.magic, abi::BOOTINFO_MAGIC);
    assert_eq!(raw.version, abi::ABI_VERSION);
    assert_eq!(raw.size, core::mem::size_of::<abi::BootInfo>() as u64);
    assert_eq!(raw.page_size, abi::PAGE_SIZE);
    assert_eq!(raw.ipc_buffer, address - abi::PAGE_SIZE);
    assert_eq!(raw.extra, address + abi::PAGE_SIZE);
    assert!(
        raw.extra
            .checked_add(raw.extra_size)
            .is_some_and(|end| end <= abi::USER_ADDRESS_LIMIT)
    );
    let header_size = core::mem::size_of::<abi::BootInfoHeader>() as u64;
    // FDT record.
    // SAFETY: the boot contract supplies a pinned, read-only extra BootInfo mapping.
    let fdt_header = unsafe { &*(raw.extra as *const abi::BootInfoHeader) };
    assert_eq!(fdt_header.id, abi::BOOTINFO_HEADER_FDT);
    assert!((header_size + 40..=header_size + abi::MAX_DTB_SIZE).contains(&fdt_header.len));
    let dtb = unsafe {
        core::slice::from_raw_parts(
            (raw.extra + header_size) as *const u8,
            (fdt_header.len - header_size) as usize,
        )
    };
    // Untyped descriptor list follows the FDT record.
    let untyped_record = (raw.extra + fdt_header.len).next_multiple_of(8);
    // SAFETY: both records live in the same validated extra mapping.
    let untyped_header = unsafe { &*(untyped_record as *const abi::BootInfoHeader) };
    assert_eq!(untyped_header.id, abi::BOOTINFO_HEADER_UNTYPED);
    let untyped_bytes = untyped_header
        .len
        .checked_sub(header_size)
        .expect("Untyped record length");
    assert_eq!(
        untyped_bytes % core::mem::size_of::<abi::UntypedDesc>() as u64,
        0
    );
    let untyped = unsafe {
        core::slice::from_raw_parts(
            (untyped_record + header_size) as *const abi::UntypedDesc,
            (untyped_bytes / core::mem::size_of::<abi::UntypedDesc>() as u64) as usize,
        )
    };
    assert_eq!(untyped.len() as u64, raw.untyped_count);
    // Optional boot-module record; a zero physical base means the bootloader
    // shipped no module archive.
    let mut modules = abi::BootModules {
        paddr: 0,
        size: 0,
        frame_start: 0,
        frame_count: 0,
        reserved: [0; 4],
    };
    let modules_offset = (untyped_record + untyped_header.len).next_multiple_of(8);
    let mut modules_end = untyped_record + untyped_header.len;
    if modules_offset + header_size <= raw.extra + raw.extra_size {
        // SAFETY: the record lives inside the validated extra mapping.
        let header = unsafe { &*(modules_offset as *const abi::BootInfoHeader) };
        if header.id == abi::BOOTINFO_HEADER_BOOT_MODULES
            && header.len == header_size + abi::BootModules::RECORD_LEN
            && modules_offset + header.len <= raw.extra + raw.extra_size
        {
            // SAFETY: fixed-size, fully initialized repr(C) record.
            modules = unsafe {
                core::ptr::read((modules_offset + header_size) as *const abi::BootModules)
            };
            modules_end = modules_offset + header.len as u64;
        }
    }
    // Optional platform IRQ table record (docs/irq.md §3.1).
    let mut irqs: &'static [abi::IrqDesc] = &[];
    let irqs_offset = modules_end.next_multiple_of(8);
    if irqs_offset + header_size <= raw.extra + raw.extra_size {
        // SAFETY: the record lives inside the validated extra mapping.
        let header = unsafe { &*(irqs_offset as *const abi::BootInfoHeader) };
        if header.id == abi::BOOTINFO_HEADER_IRQS
            && header.len >= header_size
            && (header.len - header_size) % abi::IrqDesc::RECORD_LEN == 0
            && irqs_offset + header.len <= raw.extra + raw.extra_size
        {
            irqs = unsafe {
                core::slice::from_raw_parts(
                    (irqs_offset + header_size) as *const abi::IrqDesc,
                    ((header.len - header_size) / abi::IrqDesc::RECORD_LEN) as usize,
                )
            };
        }
    }
    main(&mut BootInfo {
        raw,
        dtb,
        untyped,
        modules,
        irqs,
    })
}

pub use rstiny_runtime_macros::entry;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    rstiny::debug_println!("[user panic] {}", info);
    rstiny::suspend_self()
}

/// Remove the runtime-owned guard before entering Rust application code.
/// # Safety
/// `guard` must identify an exclusively owned, mapped page with no live
/// references or active stack bytes, reserved by the entry macro.
#[doc(hidden)]
pub unsafe fn protect_stack(guard: usize) {
    unsafe {
        rstiny::Task::current()
            .expect("root task")
            .unmap(guard, abi::PAGE_SIZE as usize)
    }
    .expect("root stack guard");
}
