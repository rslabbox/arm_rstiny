#![no_std]
//! Wire contracts between supervisors, services and clients. Every message is
//! endpoint IPC: label selects the operation, message registers carry the
//! payload. Fault labels own 0..=4, so protocol labels start at 0x100
//! (see docs/service-manager.md, decision 14).

/// Parameter page the supervisor maps into a spawned child; its address is
/// the child's x0 start argument.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct SpawnInfo {
    pub magic: u64,
    pub version: u64,
    /// Badged control endpoint: fault delivery, READY/REPORT/EXIT.
    pub control_ep: u64,
    /// Supervisor command endpoint for STOP/PING (0 = none).
    pub command_ep: u64,
    /// Child's own Untyped budget (0 = none granted).
    pub untyped: u64,
    /// Boot module ROM Frame capabilities (0 = none).
    pub rom_start: u64,
    pub rom_count: u64,
    /// Service endpoints granted through `extra` slots, by protocol.
    pub extra: [u64; 8],
}
impl SpawnInfo {
    pub const MAGIC: u64 = 0x0000_5253_5449_4e49;
    pub const VERSION: u64 = 1;
    /// `extra[0]`: console client endpoint.
    pub const CONSOLE_EP: usize = 0;
    /// `extra[1]`: first device Untyped slot (ascending physical order).
    pub const DEVICE_SLOT: usize = 1;
    /// `extra[2]`: this service's own endpoint (its server loop listens here).
    pub const SELF_EP: usize = 2;
    /// `extra[3]`: number of granted device Untyped capabilities.
    pub const DEVICE_COUNT: usize = 3;
    /// `extra[4]`: number of dependency endpoints that follow.
    pub const DEP_COUNT: usize = 4;
    /// `extra[5..]`: dependency service endpoints in `depends` order.
    pub const DEP_EP_BASE: usize = 5;
}

pub const PAGE_SIZE: u64 = kernel_abi::PAGE_SIZE;

/// Supervisor control protocol on `control_ep` (badge = service identity).
pub mod control {
    pub const READY: u64 = 0x100;
    pub const REPORT: u64 = 0x101;
    pub const EXIT: u64 = 0x102;
    pub const STOP_ACK: u64 = 0x103;
    pub const STOP: u64 = 0x104;
    pub const PING: u64 = 0x105;
    pub const DEPENDENCY_LOST: u64 = 0x106;
}

/// Console service protocol on `console_ep`. `CONSOLE_WRITE` packs `n` bytes
/// little-endian into the message registers following `n`.
pub mod console {
    pub const BIND: u64 = 1;
    pub const WRITE: u64 = 2;
    /// Maximum inline bytes of one `CONSOLE_WRITE` (14 registers of payload).
    pub const MAX_WRITE: usize = 14 * 8;
}

/// Shared status codes for the block and fs reply payloads.
pub mod status {
    pub const OK: u64 = 0;
    pub const ERROR: u64 = 1;
}

/// Block service protocol on `block_ep` (docs/disk-driver.md section 8.1).
/// One client at a time; payloads travel through the shared buffer frame the
/// server grants during `BIND`.
pub mod block {
    pub const PROTOCOL_VERSION: u64 = 1;
    /// mr0 = version; grants the shared buffer frame to the client. Reply:
    /// mr0 = version, mr1 = capacity in sectors, mr2 = sector size.
    pub const BIND: u64 = 0x100;
    /// mr0 = lba, mr1 = sector count. Reply: mr0 = status, mr1 = sectors read.
    pub const READ: u64 = 0x101;
    /// Reply: mr0 = total sectors.
    pub const CAPACITY: u64 = 0x102;
    /// Reply: mr0 = sector size, mr1 = max sectors per read.
    pub const INFO: u64 = 0x103;
}

/// Read-only filesystem protocol on `fs_ep` (docs/disk-driver.md section 8.2).
/// File payloads travel through the shared buffer frame granted in `BIND`.
pub mod fs {
    pub const PROTOCOL_VERSION: u64 = 1;
    /// mr0 = version. Reply: mr0 = version, mr1 = buffer bytes.
    pub const BIND: u64 = 0x100;
    /// mr0 = name length, mr1 = packed 8.3 short name. Reply: mr0 = file id,
    /// mr1 = file size in bytes.
    pub const OPEN: u64 = 0x101;
    /// mr0 = file id, mr1 = offset, mr2 = length. Reply: mr0 = status,
    /// mr1 = bytes delivered into the shared buffer.
    pub const READ: u64 = 0x102;
    /// mr0 = file id. Reply: mr0 = status.
    pub const CLOSE: u64 = 0x103;
    /// mr0 = name length, mr1 = packed 8.3 short name. Reply: mr0 = size,
    /// mr1 = is_dir.
    pub const STAT: u64 = 0x104;
}

/// The loader's x0 start argument for supervised children: the parameter
/// page address. Ordinary entry points accept this as `argument: usize`.
pub type Argument = usize;
