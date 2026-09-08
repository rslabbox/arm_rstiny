#![no_std]
//! Runtime for the initial task: entry, boot contract and default panic policy.
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
pub unsafe fn start(pointer: *const (), main: fn(&mut BootInfo) -> !) -> ! {
    assert_eq!(pointer as usize, abi::BOOTINFO_VA as usize);
    let raw = unsafe { &*pointer.cast::<abi::BootInfo>() };
    assert_eq!(raw.magic, abi::BOOTINFO_MAGIC);
    assert_eq!(raw.version, abi::ABI_VERSION);
    assert_eq!(raw.size, core::mem::size_of::<abi::BootInfo>() as u64);
    assert_eq!(raw.page_size, abi::PAGE_SIZE);
    assert_eq!(raw.ipc_buffer, abi::IPC_BUFFER_VA);
    assert_eq!(raw.stack_start, abi::STACK_START);
    assert_eq!(raw.stack_end, abi::STACK_END);
    assert_eq!(raw.image_start, abi::IMAGE_START);
    assert!(raw.image_end > raw.image_start && raw.image_end <= abi::IMAGE_END);
    assert_eq!(raw.extra, abi::EXTRA_BOOTINFO_VA);
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
