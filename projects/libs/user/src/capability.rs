//! Explicit seL4-style object capabilities. Slot allocation is the caller's policy.
use super::{Error, abi, invoke};
pub use abi::{
    CNODE_BITS, INIT_ASID_POOL, INIT_CNODE, INIT_TCB, INIT_UNTYPED, INIT_VSPACE, Invocation,
    ObjectType, RIGHTS_ALL, RIGHTS_READ, RIGHTS_WRITE, VM_CACHEABLE, VM_EXECUTE_NEVER,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct CPtr(pub u64);
impl CPtr {
    /// # Safety
    /// The operation must preserve the caller's memory and execution invariants.
    pub unsafe fn call(
        self,
        method: Invocation,
        words: &[u64],
        caps: &[u64],
    ) -> Result<u64, Error> {
        invoke(self.0, method as u64, words, caps)
    }
}
pub struct Untyped(pub CPtr);
impl Untyped {
    pub fn retype(
        &self,
        kind: ObjectType,
        size_bits: u64,
        cnode: CPtr,
        slot: u64,
        count: u64,
    ) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::UntypedRetype as u64,
            &[kind as u64, size_bits, 0, 0, slot, count],
            &[cnode.0],
        )
        .map(|_| ())
    }
}
pub struct CNode(pub CPtr);
impl CNode {
    pub fn copy(&self, dest: u64, source: CPtr, slot: u64, rights: u64) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::CNodeCopy as u64,
            &[dest, 64, slot, 64, rights],
            &[source.0],
        )
        .map(|_| ())
    }
    /// Mint a copy carrying an endpoint/notification badge (`cap_data`).
    /// seL4 `updateCapData`: only an unbadged source accepts a badge.
    pub fn mint(
        &self,
        dest: u64,
        source: CPtr,
        slot: u64,
        rights: u64,
        cap_data: u64,
    ) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::CNodeMint as u64,
            &[dest, 64, slot, 64, rights, cap_data],
            &[source.0],
        )
        .map(|_| ())
    }
    /// Create an Endpoint object from this Untyped capability.
    pub fn retype_endpoint(&self, untyped: CPtr, slot: u64) -> Result<(), Error> {
        invoke(
            untyped.0,
            Invocation::UntypedRetype as u64,
            &[ObjectType::Endpoint as u64, 0, 0, 0, slot, 1],
            &[self.0.0],
        )
        .map(|_| ())
    }
    /// # Safety
    /// No live references or execution may depend on mappings removed with this cap.
    pub unsafe fn delete(&self, slot: u64) -> Result<(), Error> {
        invoke(self.0.0, Invocation::CNodeDelete as u64, &[slot, 64], &[]).map(|_| ())
    }
    /// # Safety
    /// All descendants and their mappings must be safe to revoke.
    pub unsafe fn revoke(&self, slot: u64) -> Result<(), Error> {
        invoke(self.0.0, Invocation::CNodeRevoke as u64, &[slot, 64], &[]).map(|_| ())
    }
}
pub struct Page(pub CPtr);
impl Page {
    /// # Safety
    /// The mapping and its aliases must respect the address space's Rust ownership.
    pub unsafe fn map(
        &self,
        space: CPtr,
        address: usize,
        rights: u64,
        attributes: u64,
    ) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::ArmPageMap as u64,
            &[address as u64, rights, attributes],
            &[space.0],
        )
        .map(|_| ())
    }
    /// # Safety
    /// No live reference or execution may use this mapping.
    pub unsafe fn unmap(&self) -> Result<(), Error> {
        invoke(self.0.0, Invocation::ArmPageUnmap as u64, &[], &[]).map(|_| ())
    }
}
pub struct PageTable(pub CPtr);
impl PageTable {
    pub fn map(&self, space: CPtr, address: usize) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::ArmPageTableMap as u64,
            &[address as u64, abi::VM_CACHEABLE],
            &[space.0],
        )
        .map(|_| ())
    }
}
pub struct Tcb(pub CPtr);
impl Tcb {
    pub fn suspend(&self) -> Result<(), Error> {
        invoke(self.0.0, Invocation::TcbSuspend as u64, &[], &[]).map(|_| ())
    }
    /// # Safety
    /// The configured context and all its mappings must be valid for execution.
    pub unsafe fn resume(&self) -> Result<(), Error> {
        invoke(self.0.0, Invocation::TcbResume as u64, &[], &[]).map(|_| ())
    }
    /// # Safety
    /// The new roots and IPC mapping must form a valid, exclusively managed task.
    /// `fault_ep` is a slot in the configured CSpace, resolved when a fault is
    /// delivered (0 = unmonitored).
    pub unsafe fn configure(
        &self,
        cspace: CPtr,
        vspace: CPtr,
        ipc_frame: CPtr,
        ipc_address: usize,
        fault_ep: u64,
    ) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::TcbConfigure as u64,
            &[fault_ep, 64 - abi::CNODE_BITS, 0, ipc_address as u64],
            &[cspace.0, vspace.0, ipc_frame.0],
        )
        .map(|_| ())
    }
    /// Bind an unstarted thread to a CSpace/VSpace pair without touching its
    /// IPC buffer. Both may already be in use by other threads: a thread group
    /// shares its CSpace and VSpace (docs/fault-handler.md §3). `fault_ep` is
    /// a slot in the configured CSpace (0 = unmonitored).
    ///
    /// # Safety
    /// The thread must not have run yet, and the pair must form a usable
    /// execution environment for it.
    pub unsafe fn set_space(&self, cspace: CPtr, vspace: CPtr, fault_ep: u64) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::TcbSetSpace as u64,
            &[fault_ep, 0, 0],
            &[cspace.0, vspace.0],
        )
        .map(|_| ())
    }
    /// Point an unstarted thread at its own IPC buffer: `frame` must already
    /// be mapped at `address` (1 KiB aligned) in the thread's VSpace.
    /// `address == 0` clears the buffer.
    ///
    /// # Safety
    /// The thread must not have run yet, and the mapping must stay valid for
    /// the thread's lifetime.
    pub unsafe fn set_ipc_buffer(&self, frame: CPtr, address: usize) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::TcbSetIpcBuffer as u64,
            &[address as u64],
            &[frame.0],
        )
        .map(|_| ())
    }
    /// # Safety
    /// Entry and stack must be initialized and satisfy the task's startup contract.
    pub unsafe fn write_initial_registers(
        &self,
        pc: usize,
        sp: usize,
        argument: u64,
        resume: bool,
    ) -> Result<(), Error> {
        invoke(
            self.0.0,
            Invocation::TcbWriteRegisters as u64,
            &[resume as u64, 4, pc as u64, sp as u64, 0, argument],
            &[],
        )
        .map(|_| ())
    }
}
