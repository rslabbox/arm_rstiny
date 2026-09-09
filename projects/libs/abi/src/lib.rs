#![no_std]
//! Initial-task ABI. This is not binary compatible with seL4.
pub const PAGE_SIZE: u64 = 4096;
pub const MAX_DTB_SIZE: u64 = 1024 * 1024;
pub const BOOTINFO_HEADER_FDT: u64 = 6;
pub const BOOTINFO_MAGIC: u64 = 0x5253_5449_4e59_4249;
pub const ABI_VERSION: u64 = 3;
pub const FEATURE_DEBUG_CONSOLE: u64 = 1;
mod syscall;
pub use syscall::Syscall;

pub const OK: u64 = 0;
pub const UNSUPPORTED: u64 = 1;
pub const INVALID_ARGUMENT: u64 = 2;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u64,
    pub size: u64,
    pub page_size: u64,
    pub features: u64,
    pub ipc_buffer: u64,
    pub extra: u64,
    pub extra_size: u64,
}

/// Address-space and resource limits, not an application link layout.
pub const USER_ADDRESS_LIMIT: u64 = 128 * 1024 * 1024;
pub const MAX_USER_PAGES: usize = 1024;

/// Derived placement of the initial task's kernel-provided pages.
#[derive(Clone, Copy, Debug)]
pub struct InitialTaskLayout {
    pub ipc_buffer: u64,
    pub boot_info: u64,
    pub extra: u64,
    pub extra_size: u64,
    pub end: u64,
}
impl InitialTaskLayout {
    /// The ELF controls its own stack. Only image bounds and resource limits
    /// participate in this layout; BootInfo does not describe a user stack.
    pub fn new(image: core::ops::Range<u64>, dtb_size: u64) -> Option<Self> {
        if image.start < PAGE_SIZE
            || image.start >= image.end
            || !image.start.is_multiple_of(PAGE_SIZE)
            || !image.end.is_multiple_of(PAGE_SIZE)
            || image.end - image.start > MAX_USER_PAGES as u64 * PAGE_SIZE
            || !(40..=MAX_DTB_SIZE).contains(&dtb_size)
        {
            return None;
        }
        let ipc_buffer = image.end;
        let boot_info = ipc_buffer.checked_add(PAGE_SIZE)?;
        let extra = boot_info.checked_add(PAGE_SIZE)?;
        let extra_size = dtb_size.checked_add(core::mem::size_of::<BootInfoHeader>() as u64)?;
        let extra_pages = extra_size.checked_add(PAGE_SIZE - 1)? / PAGE_SIZE;
        let end = extra.checked_add(extra_pages * PAGE_SIZE)?;
        if end > USER_ADDRESS_LIMIT {
            return None;
        }
        Some(Self {
            ipc_buffer,
            boot_info,
            extra,
            extra_size,
            end,
        })
    }
    pub fn metadata_pages(&self) -> usize {
        ((self.end - self.ipc_buffer) / PAGE_SIZE) as usize
    }
}

/// Length includes this header; payload immediately follows it.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct BootInfoHeader {
    pub id: u64,
    pub len: u64,
}

// Memory and task APIs use x0..x4 arguments; result calls return a value in x1.
pub const NO_MEMORY: u64 = 3;
pub const NOT_MAPPED: u64 = 4;
pub const ALREADY_MAPPED: u64 = 5;
pub const PERMISSION_DENIED: u64 = 6;
pub const NOT_FOUND: u64 = 7;
pub const INVALID_STATE: u64 = 8;
pub const BUSY: u64 = 9;
pub const TASK_CREATED: u64 = 0;
pub const TASK_RUNNING: u64 = 1;
pub const TASK_SUSPENDED: u64 = 2;
pub const TASK_FAULTED: u64 = 3;
pub const TASK_READY: u64 = 4;
pub const TASK_SLEEPING: u64 = 5;
pub const TASK_EXITED: u64 = 6;
pub const TASK_WAITING: u64 = 7;
