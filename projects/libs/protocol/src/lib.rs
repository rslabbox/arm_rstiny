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
    pub const CONSOLE_EP: usize = 0;
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

/// The loader's x0 start argument for supervised children: the parameter
/// page address. Ordinary entry points accept this as `argument: usize`.
pub type Argument = usize;
