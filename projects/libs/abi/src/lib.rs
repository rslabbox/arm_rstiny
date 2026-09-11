#![no_std]
//! Initial-task ABI. This is not binary compatible with seL4.
pub const PAGE_SIZE: u64 = 4096;
pub const MAX_DTB_SIZE: u64 = 1024 * 1024;
pub const BOOTINFO_HEADER_FDT: u64 = 6;
pub const BOOTINFO_HEADER_UNTYPED: u64 = 7;
/// Boot module archive: physical range plus the root task's Frame caps for it.
pub const BOOTINFO_HEADER_BOOT_MODULES: u64 = 8;
pub const BOOTINFO_MAGIC: u64 = 0x5253_5449_4e59_4249;
pub const ABI_VERSION: u64 = 6;
pub const FEATURE_DEBUG_CONSOLE: u64 = 1;
/// Extra BootInfo record holding the platform's user-authorizable IRQ lines.
pub const BOOTINFO_HEADER_IRQS: u64 = 9;
mod syscall;
pub use syscall::Syscall;
mod message;
mod object;
pub use message::*;
pub use object::*;

// seL4 error labels, encoded in the reply MessageInfo.label.
pub const OK: u64 = 0;
pub const INVALID_ARGUMENT: u64 = 1;
pub const INVALID_CAPABILITY: u64 = 2;
pub const UNSUPPORTED: u64 = 3;
pub const RANGE_ERROR: u64 = 4;
pub const ALIGNMENT_ERROR: u64 = 5;
pub const NOT_FOUND: u64 = 6;
pub const TRUNCATED_MESSAGE: u64 = 7;
pub const ALREADY_MAPPED: u64 = 8;
pub const REVOKE_FIRST: u64 = 9;
pub const NO_MEMORY: u64 = 10;
// Internal errors collapse into their seL4 wire categories.
pub const NOT_MAPPED: u64 = NOT_FOUND;
pub const PERMISSION_DENIED: u64 = UNSUPPORTED;
pub const INVALID_STATE: u64 = UNSUPPORTED;
pub const BUSY: u64 = UNSUPPORTED;

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
    /// First CSpace slot of the contiguous initial Untyped capability range.
    pub untyped_start: u64,
    /// Number of initial Untyped capabilities.
    pub untyped_count: u64,
    pub reserved: [u64; 6],
}

/// One physical Untyped region published to the root task. `size_bits` is the
/// base-2 log of the region size; the region is aligned to that size.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct UntypedDesc {
    pub paddr: u64,
    pub size_bits: u64,
    pub is_device: u64,
    pub reserved: u64,
}

/// Boot module archive published to the root task (BootInfo record 8). The
/// kernel installs one read-only `Frame` capability per archive page in the
/// initial CNode starting at `frame_start`; the root task maps them itself.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct BootModules {
    pub paddr: u64,
    pub size: u64,
    pub frame_start: u64,
    pub frame_count: u64,
    pub reserved: [u64; 4],
}

impl BootModules {
    pub const RECORD_LEN: u64 = core::mem::size_of::<Self>() as u64;
}

/// Line groups of a platform IRQ table entry. VirtIO slot lines come first in
/// the table (ascending slot order) so a supervisor can map a device ordinal
/// to a line positionally; other devices' lines follow.
pub const IRQ_KIND_VIRTIO_SLOT: u64 = 0;
pub const IRQ_KIND_DEVICE: u64 = 1;

/// One user-authorizable interrupt line, published to the root task in the
/// extra BootInfo record `BOOTINFO_HEADER_IRQS` (docs/irq.md §3.1). The table
/// is generated from the DTB; authorization policy stays kernel-owned.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct IrqDesc {
    pub intid: u64,
    /// Non-zero for a level-triggered line.
    pub level: u64,
    pub kind: u64,
    pub reserved: u64,
}
impl IrqDesc {
    pub const RECORD_LEN: u64 = core::mem::size_of::<Self>() as u64;
}

/// Upper bound on platform IRQ lines published to the root task. Reserves the
/// worst-case record extent in the BootInfo layout before the partitioner runs.
pub const MAX_IRQ_LINES: usize = 64;

/// Address-space and resource limits, not an application link layout.
pub const USER_ADDRESS_LIMIT: u64 = 128 * 1024 * 1024;
pub const MAX_USER_PAGES: usize = 1024;
/// First initial CNode slot reserved for boot-module Frame capabilities.
/// High enough that the contiguous window never collides with the fixed
/// initial caps or the Untyped range that follows them.
pub const INIT_BOOT_MODULES: u64 = 512;

/// Derived placement of the initial task's kernel-provided pages.
#[derive(Clone, Copy, Debug)]
pub struct InitialTaskLayout {
    pub ipc_buffer: u64,
    pub boot_info: u64,
    pub extra: u64,
    pub extra_size: u64,
    pub end: u64,
}
/// Upper bound on boot-partitioned Untyped regions published to the root task.
/// The extra BootInfo region reserves the maximum so the loader and kernel
/// agree on the metadata layout before the partitioner runs.
pub const MAX_UNTYPED_REGIONS: usize = 64;
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
        let header = core::mem::size_of::<BootInfoHeader>() as u64;
        // FDT record, the worst-case Untyped list record, the fixed-size
        // boot-module record and the worst-case platform IRQ table record.
        let untyped_bytes = (MAX_UNTYPED_REGIONS * core::mem::size_of::<UntypedDesc>()) as u64;
        let irq_bytes = (MAX_IRQ_LINES * core::mem::size_of::<IrqDesc>()) as u64;
        let extra_size = dtb_size
            .checked_add(header)?
            .checked_add(header)?
            .checked_add(untyped_bytes)?
            .checked_add(header)?
            .checked_add(BootModules::RECORD_LEN)?
            .checked_add(header)?
            .checked_add(irq_bytes)?;
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

pub const TASK_CREATED: u64 = 0;
pub const TASK_RUNNING: u64 = 1;
pub const TASK_SUSPENDED: u64 = 2;
pub const TASK_FAULTED: u64 = 3;
pub const TASK_READY: u64 = 4;
pub const TASK_SLEEPING: u64 = 5;
pub const TASK_EXITED: u64 = 6;
pub const TASK_WAITING: u64 = 7;
/// Recoverable IPC blocking states. Unlike `TASK_FAULTED` they are supervised,
/// not terminal: a fault-endpoint reply or supervisor repair resumes them.
pub const TASK_BLOCKED_SEND: u64 = 8;
pub const TASK_BLOCKED_RECV: u64 = 9;
pub const TASK_BLOCKED_REPLY: u64 = 10;
pub const TASK_BLOCKED_FAULT: u64 = 11;

/// Fault message labels, delivered on a TCB fault endpoint. Values follow the
/// libsel4 faults.xml order for the non-hypervisor AArch64 configuration; user
/// protocol labels must start above this range (see docs/service-manager.md).
pub const FAULT_NULL: u64 = 0;
pub const FAULT_CAP: u64 = 1;
pub const FAULT_UNKNOWN_SYSCALL: u64 = 2;
pub const FAULT_USER_EXCEPTION: u64 = 3;
pub const FAULT_VM: u64 = 4;
