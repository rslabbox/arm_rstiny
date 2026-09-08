#![no_std]
//! Initial-task ABI. This is not binary compatible with seL4.
pub const PAGE_SIZE: u64 = 4096;
pub const IMAGE_START: u64 = 0x0040_0000;
pub const IMAGE_END: u64 = 0x0050_0000;
pub const BOOTINFO_VA: u64 = 0x005f_8000;
pub const IPC_BUFFER_VA: u64 = 0x005f_9000;
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
