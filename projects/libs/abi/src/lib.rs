#![no_std]
//! Initial-task ABI. This is not binary compatible with seL4.
pub const PAGE_SIZE: u64 = 4096;
pub const IMAGE_START: u64 = 0x0040_0000;
pub const IMAGE_END: u64 = 0x0060_0000;
pub const BOOTINFO_VA: u64 = 0x0060_1000;
pub const IPC_BUFFER_VA: u64 = 0x0060_0000;
pub const STACK_START: u64 = 0x005f_c000;
pub const STACK_END: u64 = 0x0060_0000;
pub const BOOTINFO_MAGIC: u64 = 0x5253_5449_4e59_4249;
pub const ABI_VERSION: u64 = 1;
pub const FEATURE_DEBUG_CONSOLE: u64 = 1;
pub const SYS_YIELD: u64 = 0;
pub const SYS_DEBUG_PUTCHAR: u64 = 1;
pub const SYS_SUSPEND_SELF: u64 = 2;
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
    pub image_start: u64,
    pub image_end: u64,
    pub stack_start: u64,
    pub stack_end: u64,
}

// Memory and task APIs use x0..x4 arguments; result calls return a value in x1.
pub const NO_MEMORY: u64 = 3;
pub const NOT_MAPPED: u64 = 4;
pub const ALREADY_MAPPED: u64 = 5;
pub const PERMISSION_DENIED: u64 = 6;
pub const NOT_FOUND: u64 = 7;
pub const INVALID_STATE: u64 = 8;
pub const BUSY: u64 = 9;
pub const SYS_TASK_ID: u64 = 3;
pub const SYS_TASK_CREATE: u64 = 4;
pub const SYS_TASK_START: u64 = 5;
pub const SYS_TASK_SUSPEND: u64 = 6;
pub const SYS_TASK_RESUME: u64 = 7;
pub const SYS_TASK_DESTROY: u64 = 8;
pub const SYS_TASK_STATUS: u64 = 9;
pub const SYS_EXIT: u64 = 10;
pub const SYS_SLEEP: u64 = 11;
pub const SYS_MAP: u64 = 12;
pub const SYS_UNMAP: u64 = 13;
pub const SYS_PROTECT: u64 = 14;
pub const SYS_WRITE_MEMORY: u64 = 15;
pub const SYS_READ_MEMORY: u64 = 16;
pub const SYS_MEMORY_AVAILABLE: u64 = 17;
pub const SYS_CLOCK: u64 = 18;
pub const SYS_WAIT: u64 = 19;
pub const TASK_CREATED: u64 = 0;
pub const TASK_RUNNING: u64 = 1;
pub const TASK_SUSPENDED: u64 = 2;
pub const TASK_FAULTED: u64 = 3;
pub const TASK_READY: u64 = 4;
pub const TASK_SLEEPING: u64 = 5;
pub const TASK_EXITED: u64 = 6;
pub const TASK_WAITING: u64 = 7;
