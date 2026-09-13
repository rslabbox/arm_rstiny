//! Task handles. Destruction is a standard capability operation: the loader's
//! derivation subtree is revoked, which stops every thread and reclaims every
//! object the child ever created (docs/capability-authority-untyped.md §3.4).
use super::{Error, abi, capability::CNode, capability::CPtr, invoke, runtime};

#[derive(Debug, PartialEq, Eq)]
pub struct Task(u64, Option<u64>);

impl Task {
    pub fn current() -> Result<Self, Error> {
        runtime(abi::RuntimeInvocation::Current, &[]).map(|cap| Self(cap, None))
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
    /// Terminate and reap the task by revoking its loader derivation subtree:
    /// every thread sharing the child's CSpace, every object it created and
    /// every mapping it holds die with the budget they were carved from.
    /// Released CSpace slots may subsequently be reused.
    pub fn destroy(self) -> Result<(), Error> {
        let Some(allocator) = self.1 else {
            return Err(Error::InvalidArgument);
        };
        // Best effort: an already terminal task has nothing left to suspend.
        let _ = self.operation(abi::Invocation::TcbSuspend);
        // SAFETY: this handle uniquely owns the derivation subtree; the task
        // is stopped and nothing else references it.
        unsafe {
            let cnode = CNode(CPtr(abi::INIT_CNODE));
            cnode.revoke(allocator)?;
            cnode.delete(allocator)?;
        }
        Ok(())
    }
    /// Terminate exactly this thread: stop it and delete its TCB capability,
    /// which lets collection reap the thread. A shared CSpace/VSpace survive
    /// for the sibling threads of its group (docs/thread-group.md §2.2); the
    /// caller releases the thread's own caps with `Thread::release`.
    pub fn destroy_thread(self) -> Result<(), Error> {
        // Best effort: an already terminal thread has nothing left to suspend.
        let _ = self.operation(abi::Invocation::TcbSuspend);
        // SAFETY: the thread is stopped; its TCB object becomes collectable
        // and no live execution depends on the slot.
        unsafe { CNode(CPtr(abi::INIT_CNODE)).delete(self.0) }
    }
    pub(super) fn from_objects(tcb: u64, allocator: u64) -> Self {
        Self(tcb, Some(allocator))
    }
    /// Wrap an existing TCB capability slot, e.g. a thread-group member's.
    pub fn from_tcb(slot: u64) -> Self {
        Self(slot, None)
    }
}
