//! Managed-runtime task capabilities. These convenience services extend seL4.
use super::{Error, abi, invoke, runtime};

#[derive(Debug, PartialEq, Eq)]
pub struct Task(u64, Option<u64>);
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
        runtime(abi::RuntimeInvocation::Current, &[]).map(|cap| Self(cap, None))
    }
    /// Create a stopped child with a private address space and initialized IPC page.
    pub fn create() -> Result<Self, Error> {
        runtime(abi::RuntimeInvocation::Create, &[]).map(|cap| Self(cap, None))
    }
    /// Capability slot in the current CSpace, not a global task identity.
    pub fn id(&self) -> u64 {
        self.0
    }
    fn operation(&self, method: abi::Invocation) -> Result<(), Error> {
        invoke(self.0, method as u64, &[], &[]).map(|_| ())
    }
    pub fn suspend(&self) -> Result<(), Error> {
        self.operation(abi::Invocation::TcbSuspend)
    }
    pub fn resume(&self) -> Result<(), Error> {
        self.operation(abi::Invocation::TcbResume)
    }
    /// Terminate and reap the task, revoking its ELF object group when present.
    /// Released CSpace slots may subsequently be reused.
    pub fn destroy(self) -> Result<(), Error> {
        if let Some(allocator) = self.1 {
            // Finish the scheduler task first, while its TCB capability is
            // still valid, then reclaim the loader's derivation subtree.
            runtime(abi::RuntimeInvocation::Destroy, &[self.0])?;
            // SAFETY: this handle uniquely owns the derivation subtree; the
            // task is terminated and nothing else references it.
            unsafe {
                let cnode = super::capability::CNode(super::capability::CPtr(abi::INIT_CNODE));
                cnode.revoke(allocator)?;
                cnode.delete(allocator)?;
            }
            Ok(())
        } else {
            runtime(abi::RuntimeInvocation::Destroy, &[self.0]).map(|_| ())
        }
    }
    pub(super) fn from_objects(tcb: u64, allocator: u64) -> Self {
        Self(tcb, Some(allocator))
    }
    /// Wait for target termination. Does not reap; inspect status to distinguish faults.
    pub fn wait(&self) -> Result<u64, Error> {
        runtime(abi::RuntimeInvocation::Wait, &[self.0])
    }
    pub fn status(&self) -> Result<TaskState, Error> {
        Ok(match runtime(abi::RuntimeInvocation::Status, &[self.0])? {
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
    pub unsafe fn start(&self, entry: usize, stack: usize, argument: u64) -> Result<(), Error> {
        runtime(
            abi::RuntimeInvocation::Start,
            &[self.0, entry as u64, stack as u64, argument],
        )
        .map(|_| ())
    }
    /// Map zero-filled pages. No mapping is replaced on overlap or allocation failure.
    /// # Safety
    /// The mapping must agree with the task's runtime and pointer ownership model.
    pub unsafe fn map(
        &self,
        address: usize,
        length: usize,
        rights: Permissions,
    ) -> Result<(), Error> {
        runtime(
            abi::RuntimeInvocation::Map,
            &[self.0, address as u64, length as u64, rights as u64],
        )
        .map(|_| ())
    }
    /// # Safety
    /// No live references, stack, or executable continuation may require this range.
    pub unsafe fn unmap(&self, address: usize, length: usize) -> Result<(), Error> {
        runtime(
            abi::RuntimeInvocation::Unmap,
            &[self.0, address as u64, length as u64],
        )
        .map(|_| ())
    }
    /// # Safety
    /// All live references and executable continuations must permit the new rights.
    pub unsafe fn protect(
        &self,
        address: usize,
        length: usize,
        rights: Permissions,
    ) -> Result<(), Error> {
        runtime(
            abi::RuntimeInvocation::Protect,
            &[self.0, address as u64, length as u64, rights as u64],
        )
        .map(|_| ())
    }
    /// Copy up to 4096 bytes into a writable mapping of a stopped child or self.
    /// # Safety
    /// Writing the destination must not violate existing Rust reference invariants.
    pub unsafe fn write_memory(&self, address: usize, data: &[u8]) -> Result<(), Error> {
        runtime(
            abi::RuntimeInvocation::WriteMemory,
            &[
                self.0,
                address as u64,
                data.as_ptr() as u64,
                data.len() as u64,
            ],
        )
        .map(|_| ())
    }
    /// Read up to 4096 bytes from a stopped child or self into a Rust buffer.
    pub fn read_memory(&self, address: usize, data: &mut [u8]) -> Result<(), Error> {
        runtime(
            abi::RuntimeInvocation::ReadMemory,
            &[
                self.0,
                address as u64,
                data.as_mut_ptr() as u64,
                data.len() as u64,
            ],
        )
        .map(|_| ())
    }
}
