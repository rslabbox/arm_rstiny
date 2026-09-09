//! System call numbers shared by the kernel and userspace.
/// The AArch64 ABI passes this number in x8 and up to five arguments in x0..x4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Syscall {
    Yield = 0,
    DebugPutchar = 1,
    SuspendSelf = 2,
    TaskId = 3,
    TaskCreate = 4,
    TaskStart = 5,
    TaskSuspend = 6,
    TaskResume = 7,
    TaskDestroy = 8,
    TaskStatus = 9,
    Exit = 10,
    Sleep = 11,
    Map = 12,
    Unmap = 13,
    Protect = 14,
    WriteMemory = 15,
    ReadMemory = 16,
    MemoryAvailable = 17,
    Clock = 18,
    Wait = 19,
}

impl TryFrom<u64> for Syscall {
    /// An unrecognized number, preserved for the caller.
    type Error = u64;

    fn try_from(number: u64) -> Result<Self, Self::Error> {
        match number {
            0 => Ok(Self::Yield),
            1 => Ok(Self::DebugPutchar),
            2 => Ok(Self::SuspendSelf),
            3 => Ok(Self::TaskId),
            4 => Ok(Self::TaskCreate),
            5 => Ok(Self::TaskStart),
            6 => Ok(Self::TaskSuspend),
            7 => Ok(Self::TaskResume),
            8 => Ok(Self::TaskDestroy),
            9 => Ok(Self::TaskStatus),
            10 => Ok(Self::Exit),
            11 => Ok(Self::Sleep),
            12 => Ok(Self::Map),
            13 => Ok(Self::Unmap),
            14 => Ok(Self::Protect),
            15 => Ok(Self::WriteMemory),
            16 => Ok(Self::ReadMemory),
            17 => Ok(Self::MemoryAvailable),
            18 => Ok(Self::Clock),
            19 => Ok(Self::Wait),
            _ => Err(number),
        }
    }
}
