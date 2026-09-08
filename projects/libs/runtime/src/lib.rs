#![no_std]
//! Root and ordinary task entrypoints, boot contract and default panic policy.
use kernel_abi as abi;

/// Validated boot data and exclusive access to the initial IPC buffer.
/// Constructed once by the runtime; it cannot be cloned or built by applications.
pub struct BootInfo {
    raw: &'static abi::BootInfo,
    ipc: &'static mut [u64],
    dtb: &'static [u8],
}

impl BootInfo {
    /// Read-only DTB forwarded by the kernel. The application chooses its parser.
    pub fn device_tree(&self) -> &'static [u8] {
        self.dtb
    }

    pub fn address(&self) -> usize {
        self.raw as *const _ as usize
    }

    pub fn debug_console_available(&self) -> bool {
        self.raw.features & abi::FEATURE_DEBUG_CONSOLE != 0
    }

    /// Scratch space for now. A future IPC API must borrow this buffer while
    /// the kernel may access it, rather than keeping a second mutable reference.
    pub fn ipc_buffer(&mut self) -> &mut [u64] {
        self.ipc
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
    assert!((header_size + 40..=header_size + abi::MAX_DTB_SIZE).contains(&raw.extra_size));
    // SAFETY: the boot contract supplies a pinned, read-only extra BootInfo mapping.
    let header = unsafe { &*(raw.extra as *const abi::BootInfoHeader) };
    assert_eq!(header.id, abi::BOOTINFO_HEADER_FDT);
    assert_eq!(header.len, raw.extra_size);
    let dtb = unsafe {
        core::slice::from_raw_parts(
            (raw.extra + header_size) as *const u8,
            (raw.extra_size - header_size) as usize,
        )
    };
    let ipc = unsafe {
        core::slice::from_raw_parts_mut(raw.ipc_buffer as *mut u64, abi::PAGE_SIZE as usize / 8)
    };
    main(&mut BootInfo { raw, ipc, dtb })
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
