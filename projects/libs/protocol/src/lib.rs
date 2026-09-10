#![no_std]
//! Wire contracts between supervisors, services and clients. Every message is
//! endpoint IPC: `MessageInfo.label` selects the operation and the message
//! registers carry the payload.
//!
//! # Label space
//!
//! seL4 assigns each object type (protocol) a contiguous run of invocation
//! labels, generated from the interface XML (`enum invocation_label`). User
//! protocols share the same 52-bit label field as fault messages, so they must
//! stay clear of the fault labels and of one another.
//!
//! - fault labels own `0..=4`;
//! - each user protocol owns a [`SEGMENT_SIZE`]-wide segment starting at its
//!   `BASE` (console `0x100`, control `0x200`, internal `0x300`, block `0x400`,
//!   fs `0x500`);
//! - the kernel Runtime extension owns `0x1000..=0x10ff`.
//!
//! Segments must never overlap: a service that handles two protocols on one
//! endpoint (its own protocol plus `control`) would otherwise shadow one of
//! them. [`SEGMENTS`]/[`LABELS`] expose the layout to the host test in
//! `tests/segments.rs`; the `const _` block below rejects overlaps at compile
//! time.

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

/// Width of one protocol's label segment. Every label of a protocol lives in
/// `[BASE, BASE + SEGMENT_SIZE)`.
pub const SEGMENT_SIZE: u64 = 0x100;

/// Supervisor control protocol on `control_ep` (badge = service identity).
pub mod control {
    pub const BASE: u64 = 0x200;
    pub const READY: u64 = BASE + 0x00;
    pub const REPORT: u64 = BASE + 0x01;
    pub const EXIT: u64 = BASE + 0x02;
    pub const STOP_ACK: u64 = BASE + 0x03;
    pub const STOP: u64 = BASE + 0x04;
    pub const PING: u64 = BASE + 0x05;
    pub const DEPENDENCY_LOST: u64 = BASE + 0x06;
}

/// Console service protocol on `console_ep`. `CONSOLE_WRITE` packs `n` bytes
/// little-endian into the message registers following `n`.
pub mod console {
    pub const BASE: u64 = 0x100;
    pub const BIND: u64 = BASE + 0x00;
    pub const WRITE: u64 = BASE + 0x01;
    /// Maximum inline bytes of one `CONSOLE_WRITE` (14 registers of payload).
    pub const MAX_WRITE: usize = 14 * 8;
}

/// init's internal supervisor→logger channel. Not a service protocol: the two
/// threads of init's own thread group exchange log lines here.
pub mod internal {
    pub const BASE: u64 = 0x300;
    /// Supervisor → logger: `[len, flags, payload…]`.
    pub const POST: u64 = BASE + 0x00;
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
    pub const BASE: u64 = 0x400;
    pub const PROTOCOL_VERSION: u64 = 1;
    /// mr0 = version; grants the shared buffer frame to the client. Reply:
    /// mr0 = version, mr1 = capacity in sectors, mr2 = sector size.
    pub const BIND: u64 = BASE + 0x00;
    /// mr0 = lba, mr1 = sector count. Reply: mr0 = status, mr1 = sectors read.
    pub const READ: u64 = BASE + 0x01;
    /// Reply: mr0 = total sectors.
    pub const CAPACITY: u64 = BASE + 0x02;
    /// Reply: mr0 = sector size, mr1 = max sectors per read.
    pub const INFO: u64 = BASE + 0x03;
}

/// Read-only filesystem protocol on `fs_ep` (docs/disk-driver.md section 8.2).
/// File payloads travel through the shared buffer frame granted in `BIND`.
pub mod fs {
    pub const BASE: u64 = 0x500;
    pub const PROTOCOL_VERSION: u64 = 1;
    /// mr0 = version. Reply: mr0 = version, mr1 = buffer bytes.
    pub const BIND: u64 = BASE + 0x00;
    /// mr0 = name length, mr1 = packed 8.3 short name. Reply: mr0 = file id,
    /// mr1 = file size in bytes.
    pub const OPEN: u64 = BASE + 0x01;
    /// mr0 = file id, mr1 = offset, mr2 = length. Reply: mr0 = status,
    /// mr1 = bytes delivered into the shared buffer.
    pub const READ: u64 = BASE + 0x02;
    /// mr0 = file id. Reply: mr0 = status.
    pub const CLOSE: u64 = BASE + 0x03;
    /// mr0 = name length, mr1 = packed 8.3 short name. Reply: mr0 = size,
    /// mr1 = is_dir.
    pub const STAT: u64 = BASE + 0x04;
}

/// Declared protocol segments, for the disjointness test.
pub const SEGMENTS: &[(&str, u64)] = &[
    ("console", console::BASE),
    ("control", control::BASE),
    ("internal", internal::BASE),
    ("block", block::BASE),
    ("fs", fs::BASE),
];

/// Every defined label with the segment base it must fall inside.
pub const LABELS: &[(&str, u64, u64)] = &[
    ("console::BIND", console::BIND, console::BASE),
    ("console::WRITE", console::WRITE, console::BASE),
    ("control::READY", control::READY, control::BASE),
    ("control::REPORT", control::REPORT, control::BASE),
    ("control::EXIT", control::EXIT, control::BASE),
    ("control::STOP_ACK", control::STOP_ACK, control::BASE),
    ("control::STOP", control::STOP, control::BASE),
    ("control::PING", control::PING, control::BASE),
    (
        "control::DEPENDENCY_LOST",
        control::DEPENDENCY_LOST,
        control::BASE,
    ),
    ("internal::POST", internal::POST, internal::BASE),
    ("block::BIND", block::BIND, block::BASE),
    ("block::READ", block::READ, block::BASE),
    ("block::CAPACITY", block::CAPACITY, block::BASE),
    ("block::INFO", block::INFO, block::BASE),
    ("fs::BIND", fs::BIND, fs::BASE),
    ("fs::OPEN", fs::OPEN, fs::BASE),
    ("fs::READ", fs::READ, fs::BASE),
    ("fs::CLOSE", fs::CLOSE, fs::BASE),
    ("fs::STAT", fs::STAT, fs::BASE),
];

// Compile-time proof that the segments are disjoint: each base starts at or
// after the end of the previous segment. List order must stay ascending.
const _: () = {
    assert!(console::BASE + SEGMENT_SIZE <= control::BASE);
    assert!(control::BASE + SEGMENT_SIZE <= internal::BASE);
    assert!(internal::BASE + SEGMENT_SIZE <= block::BASE);
    assert!(block::BASE + SEGMENT_SIZE <= fs::BASE);
    // The kernel Runtime extension owns its own segment at 0x1000.
    assert!(fs::BASE + SEGMENT_SIZE <= 0x1000);
};

/// The loader's x0 start argument for supervised children: the parameter
/// page address. Ordinary entry points accept this as `argument: usize`.
pub type Argument = usize;
