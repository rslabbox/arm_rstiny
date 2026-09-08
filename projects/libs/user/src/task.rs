//! Owned-child task handles. Kernel authorization is checked on every operation.
use super::{Error, abi, invoke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Permissions {
    Read = 1,
    ReadWrite = 3,
    ReadExecute = 5,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Created,
    Running,
    Suspended,
    Faulted,
    Ready,
    Sleeping,
    Exited,
    Waiting,
}

impl Task {
    pub fn current() -> Result<Self, Error> {
        invoke(abi::SYS_TASK_ID, [0; 5]).map(Self)
    }
    /// Create a stopped child with an empty user address space.
    pub fn create() -> Result<Self, Error> {
        invoke(abi::SYS_TASK_CREATE, [0; 5]).map(Self)
    }
    pub fn id(self) -> u64 {
        self.0
    }
    fn operation(self, number: u64) -> Result<(), Error> {
        invoke(number, [self.0, 0, 0, 0, 0]).map(|_| ())
    }
    pub fn suspend(self) -> Result<(), Error> {
        self.operation(abi::SYS_TASK_SUSPEND)
    }
    pub fn resume(self) -> Result<(), Error> {
        self.operation(abi::SYS_TASK_RESUME)
    }
    /// Terminate and reap a child. Stale copies of its handle are rejected.
    pub fn destroy(self) -> Result<(), Error> {
        self.operation(abi::SYS_TASK_DESTROY)
    }
    /// Wait for child termination. Does not reap; inspect status to distinguish faults.
    pub fn wait(self) -> Result<u64, Error> {
        invoke(abi::SYS_WAIT, [self.0, 0, 0, 0, 0])
    }
    pub fn status(self) -> Result<TaskState, Error> {
        Ok(match invoke(abi::SYS_TASK_STATUS, [self.0, 0, 0, 0, 0])? {
            abi::TASK_CREATED => TaskState::Created,
            abi::TASK_RUNNING => TaskState::Running,
            abi::TASK_SUSPENDED => TaskState::Suspended,
            abi::TASK_FAULTED => TaskState::Faulted,
            abi::TASK_READY => TaskState::Ready,
            abi::TASK_SLEEPING => TaskState::Sleeping,
            abi::TASK_EXITED => TaskState::Exited,
            abi::TASK_WAITING => TaskState::Waiting,
            code => return Err(Error::Unknown(code)),
        })
    }
    /// Start a created task with x0=argument, other GPRs zero, and IRQs enabled.
    /// # Safety
    /// Entry and stack must implement a valid Rust/architecture startup contract;
    /// the caller must have initialized all memory required by the loaded code.
    pub unsafe fn start(self, entry: usize, stack: usize, argument: u64) -> Result<(), Error> {
        invoke(
            abi::SYS_TASK_START,
            [self.0, entry as u64, stack as u64, argument, 0],
        )
        .map(|_| ())
    }
    /// Map zero-filled pages. No mapping is replaced on overlap or allocation failure.
    /// # Safety
    /// The mapping must agree with the task's runtime and pointer ownership model.
    pub unsafe fn map(
        self,
        address: usize,
        length: usize,
        rights: Permissions,
    ) -> Result<(), Error> {
        invoke(
            abi::SYS_MAP,
            [self.0, address as u64, length as u64, rights as u64, 0],
        )
        .map(|_| ())
    }
    /// # Safety
    /// No live references, stack, or executable continuation may require this range.
    pub unsafe fn unmap(self, address: usize, length: usize) -> Result<(), Error> {
        invoke(
            abi::SYS_UNMAP,
            [self.0, address as u64, length as u64, 0, 0],
        )
        .map(|_| ())
    }
    /// # Safety
    /// All live references and executable continuations must permit the new rights.
    pub unsafe fn protect(
        self,
        address: usize,
        length: usize,
        rights: Permissions,
    ) -> Result<(), Error> {
        invoke(
            abi::SYS_PROTECT,
            [self.0, address as u64, length as u64, rights as u64, 0],
        )
        .map(|_| ())
    }
    /// Copy up to 4096 bytes into a writable mapping of a stopped child or self.
    /// # Safety
    /// Writing the destination must not violate existing Rust reference invariants.
    pub unsafe fn write_memory(self, address: usize, data: &[u8]) -> Result<(), Error> {
        invoke(
            abi::SYS_WRITE_MEMORY,
            [
                self.0,
                address as u64,
                data.as_ptr() as u64,
                data.len() as u64,
                0,
            ],
        )
        .map(|_| ())
    }
    /// Read up to 4096 bytes from a stopped child or self into a Rust buffer.
    pub fn read_memory(self, address: usize, data: &mut [u8]) -> Result<(), Error> {
        invoke(
            abi::SYS_READ_MEMORY,
            [
                self.0,
                address as u64,
                data.as_mut_ptr() as u64,
                data.len() as u64,
                0,
            ],
        )
        .map(|_| ())
    }
}
