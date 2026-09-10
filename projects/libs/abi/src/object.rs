//! Invocation labels for AArch64, non-MCS, UP, without hardware debug/SMMU/VCPU.
/// The seL4 XML declaration order defines these values; see docs/sel4-abi.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Invocation {
    UntypedRetype = 1,
    TcbReadRegisters = 2,
    TcbWriteRegisters = 3,
    TcbConfigure = 5,
    TcbSetIpcBuffer = 9,
    TcbSetSpace = 10,
    TcbSuspend = 11,
    TcbResume = 12,
    CNodeRevoke = 17,
    CNodeDelete = 18,
    CNodeCopy = 20,
    CNodeMint = 21,
    CNodeMove = 22,
    ArmPageTableMap = 38,
    ArmPageTableUnmap = 39,
    ArmPageMap = 40,
    ArmPageUnmap = 41,
    ArmPageGetAddress = 46,
    ArmVspaceTranslate = 47,
    ArmAsidPoolAssign = 48,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum ObjectType {
    Untyped = 0,
    Tcb = 1,
    Endpoint = 2,
    Notification = 3,
    CNode = 4,
    VSpace = 6,
    SmallPage = 7,
    PageTable = 9,
}
/// Well-known initial capabilities; all lookups are relative to the current CSpace.
pub const INIT_TCB: u64 = 1;
pub const INIT_CNODE: u64 = 2;
pub const INIT_VSPACE: u64 = 3;
pub const INIT_ASID_POOL: u64 = 6;
pub const INIT_IPC_BUFFER: u64 = 10;
/// First initial Untyped capability. The range continues for `BootInfo::untyped_count`
/// slots; applications should read `BootInfo::untyped_start` instead of assuming it.
pub const INIT_UNTYPED: u64 = 32;
pub const INIT_RUNTIME: u64 = 17;
pub const FIRST_FREE_SLOT: u64 = 32;
pub const CNODE_BITS: u64 = 16;
pub const CNODE_SLOTS: u64 = 1 << CNODE_BITS;
pub const RIGHTS_WRITE: u64 = 1;
pub const RIGHTS_READ: u64 = 2;
pub const RIGHTS_GRANT: u64 = 4;
pub const RIGHTS_GRANT_REPLY: u64 = 8;
pub const RIGHTS_ALL: u64 = 15;
pub const VM_CACHEABLE: u64 = 1;
pub const VM_EXECUTE_NEVER: u64 = 4;

/// Explicit RSTiny runtime extensions, never seL4 syscall or invocation numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum RuntimeInvocation {
    Current = 0x1000,
    Create = 0x1001,
    Start = 0x1002,
    Status = 0x1003,
    Destroy = 0x1004,
    Wait = 0x1005,
    Sleep = 0x1006,
    Exit = 0x1007,
    Clock = 0x1008,
    AvailableFrames = 0x1009,
    Map = 0x100a,
    Unmap = 0x100b,
    Protect = 0x100c,
    WriteMemory = 0x100d,
    ReadMemory = 0x100e,
    FindEmptySlot = 0x100f,
    DebugConsoleAvailable = 0x1010,
    Cspace = 0x1011,
    Vspace = 0x1012,
    /// Destroy exactly one thread; its shared CSpace/VSpace survive for its
    /// siblings. `Destroy` is the group-level counterpart
    /// (docs/thread-group.md §2.2).
    DestroyThread = 0x1013,
}
