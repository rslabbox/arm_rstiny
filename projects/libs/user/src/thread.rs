//! Thread groups: several TCBs sharing one CSpace and VSpace
//! (docs/fault-handler.md §3, §5).
//!
//! A group is created from an existing thread's resources: the CSpace/VSpace
//! capabilities stay shared — every member sees the same capability slots and
//! the same mappings — while each spawned thread receives its own user stack,
//! IPC buffer, kernel stack and fault endpoint from the group budget.
//!
//! Invariant (docs/fault-handler.md §7): a fault-receiving thread must never
//! issue a blocking `ipc::call` to a service it supervises. Requests that may
//! block on a service belong on a thread spawned here.

use super::{
    Error,
    capability::{CNode, CPtr, Page, PageTable, Tcb, Untyped},
};
use kernel_abi::{
    ObjectType, RIGHTS_ALL, RIGHTS_READ, RIGHTS_WRITE, VM_CACHEABLE, VM_EXECUTE_NEVER,
};

/// User stack per thread: four pages, matching the loader's 16 KiB convention.
pub const THREAD_STACK_PAGES: usize = 4;
/// One private 2 MiB region per thread, far from every other user window
/// (loader images below 0x0400_0000, ROM at 0x0200_0000, loader scratch at
/// 0x07E0_0000, managed IPC page at the address-space limit).
const THREAD_WINDOW: usize = 0x0400_0000;
const THREAD_STRIDE: usize = 0x0020_0000;
const MAX_THREADS: usize = 8;
/// Cap slots reserved per thread in the shared CNode: TCB, page table, stack
/// pages and IPC buffer frame, plus headroom for future per-thread objects.
const SLOTS_PER_THREAD: u64 = 16;
const PAGE: usize = 4096;

/// Fault supervision for a spawned thread (docs/thread-group.md §4.1): the
/// supervisor derives a badged fault-endpoint cap into the shared CSpace and
/// the thread's `fault_ep` names that slot, so its faults reach the group's
/// supervisor carrying `badge` and can be reaped and rebuilt.
pub struct FaultSupervision {
    /// Slot of the unbadged endpoint cap to mint from (same shared CSpace).
    pub source: u64,
    /// Shared-CSpace slot receiving the badged fault-endpoint cap.
    pub slot: u64,
    /// Badge delivered with this thread's faults; must not collide with
    /// service badges (e.g. `0x8000 + index`).
    pub badge: u64,
}

/// Shared resources for spawning threads into an existing address space.
pub struct ThreadGroup {
    /// Shared CSpace root: every member resolves the same capability slots.
    pub cnode: CPtr,
    /// Shared VSpace: mappings made through it are visible to every member.
    pub vspace: CPtr,
    /// Untyped budget carved for new threads' stacks and IPC buffers.
    pub untyped: CPtr,
    next_slot: u64,
    next_thread: usize,
}

/// A thread spawned into a group. Dropping it does not stop the thread; use
/// [`Thread::resume`] or take it down with `Task::destroy_thread` +
/// [`Thread::release`].
pub struct Thread {
    /// The thread's TCB capability slot in the shared CSpace.
    pub tcb: u64,
    /// Top of the thread's private user stack (grows downwards).
    pub stack_top: usize,
    /// The thread's IPC buffer address in the shared VSpace.
    pub ipc_buffer: usize,
    /// The group's CSpace root (the CNode its caps live in).
    pub cnode: CPtr,
}

impl Thread {
    pub fn handle(&self) -> Tcb {
        Tcb(CPtr(self.tcb))
    }
    /// # Safety
    /// The thread must be suspended or dead; resuming a live thread is a no-op
    /// error, but racing its state from another thread is the caller's policy.
    pub unsafe fn resume(&self) -> Result<(), Error> {
        // SAFETY: the caller guarantees the thread is not live.
        unsafe { self.handle().resume() }
    }
    /// Release the thread's per-thread capabilities (page table, stack pages,
    /// IPC buffer frame; its TCB cap is already gone after
    /// `Task::destroy_thread`). The shared CSpace/VSpace stay with the group.
    ///
    /// # Safety
    /// The thread must be terminated; nothing may reference its stack, IPC
    /// buffer or the cap slots it consumed.
    pub unsafe fn release(self) {
        let cnode = CNode(self.cnode);
        for offset in 0..SLOTS_PER_THREAD {
            // SAFETY: this thread's own caps; its execution is gone.
            unsafe {
                let _ = cnode.delete(self.tcb + offset);
            }
        }
    }
}

impl ThreadGroup {
    /// `slot_base` is the first CSpace slot used for spawned threads; each
    /// thread consumes [`SLOTS_PER_THREAD`] consecutive slots.
    pub const fn new(cnode: CPtr, vspace: CPtr, untyped: CPtr, slot_base: u64) -> Self {
        Self {
            cnode,
            vspace,
            untyped,
            next_slot: slot_base,
            next_thread: 0,
        }
    }

    /// Create and start a thread in the group: retype a TCB from the group
    /// budget, map a private stack and IPC buffer inside the shared VSpace,
    /// bind the thread to the shared CSpace/VSpace (`TCB_SetSpace` +
    /// `TCB_SetIPCBuffer`), then write its initial registers and resume it
    /// with x0 = `argument`. `fault` additionally mints the thread's badged
    /// fault-endpoint cap so its crashes are supervised (docs/thread-group.md
    /// §4.1); without it the thread crashes unmonitored (`fault_ep = 0`).
    ///
    /// # Safety
    /// `entry` must point at mapped executable code in the shared VSpace and
    /// satisfy the ordinary user startup contract; the shared address space
    /// must tolerate another thread using the fresh stack and IPC buffer.
    pub unsafe fn spawn_thread(
        &mut self,
        entry: usize,
        argument: u64,
        fault: Option<FaultSupervision>,
    ) -> Result<Thread, Error> {
        let index = self.next_thread;
        if index >= MAX_THREADS {
            return Err(Error::Range);
        }
        let slot = self.next_slot;
        self.next_thread += 1;
        self.next_slot += SLOTS_PER_THREAD;
        let fault_slot = fault.as_ref().map(|fault| fault.slot);

        let result = self.spawn_at(slot, index, entry, argument, fault);
        if let Err(error) = result {
            // Best-effort rollback: every cap created for this thread goes
            // away, unmapping its pages and letting collection reclaim them.
            let cnode = CNode(self.cnode);
            for offset in 0..SLOTS_PER_THREAD {
                // SAFETY: these slots were just created for this thread; no
                // execution or mapping outside it depends on them.
                unsafe {
                    let _ = cnode.delete(slot + offset);
                }
            }
            if let Some(fault_slot) = fault_slot {
                // SAFETY: the minted fault cap belongs to this thread; if the
                // mint never landed the slot was empty and delete is a no-op.
                unsafe {
                    let _ = cnode.delete(fault_slot);
                }
            }
            self.next_thread -= 1;
            self.next_slot -= SLOTS_PER_THREAD;
            return Err(error);
        }
        Ok(Thread {
            tcb: slot,
            stack_top: window(index) + (THREAD_STACK_PAGES + 1) * PAGE,
            ipc_buffer: window(index),
            cnode: self.cnode,
        })
    }

    fn spawn_at(
        &mut self,
        slot: u64,
        index: usize,
        entry: usize,
        argument: u64,
        fault: Option<FaultSupervision>,
    ) -> Result<(), Error> {
        let base = window(index);
        let ipc = base;
        let stack_top = base + (THREAD_STACK_PAGES + 1) * PAGE;
        let stack_bottom = base + PAGE;
        let untyped = Untyped(self.untyped);
        // The TCB is just a thread identity until SetSpace binds it; sharing
        // the group's CSpace/VSpace is the whole point of the group.
        untyped.retype(ObjectType::Tcb, 0, self.cnode, slot, 1)?;
        // All of this thread's windows sit inside one 2 MiB region, so one L3
        // table covers stack and IPC buffer.
        untyped.retype(ObjectType::PageTable, 0, self.cnode, slot + 1, 1)?;
        PageTable(CPtr(slot + 1)).map(self.vspace, base)?;
        untyped.retype(
            ObjectType::SmallPage,
            0,
            self.cnode,
            slot + 2,
            THREAD_STACK_PAGES as u64,
        )?;
        for page in 0..THREAD_STACK_PAGES {
            // SAFETY: fresh pages inside the group's own address space; the
            // caller guarantees the window is unused.
            unsafe {
                Page(CPtr(slot + 2 + page as u64)).map(
                    self.vspace,
                    stack_bottom + page * PAGE,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_CACHEABLE | VM_EXECUTE_NEVER,
                )?;
            }
        }
        untyped.retype(ObjectType::SmallPage, 0, self.cnode, slot + 6, 1)?;
        // SAFETY: the IPC buffer page is mapped writable for exactly this use.
        unsafe {
            Page(CPtr(slot + 6)).map(
                self.vspace,
                ipc,
                RIGHTS_READ | RIGHTS_WRITE,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )?;
        }
        let thread = Tcb(CPtr(slot));
        // The fault endpoint is a badged derivation of the supervisor's
        // control cap, minted into the shared CSpace before the thread runs.
        let fault_slot = match fault {
            Some(fault) => {
                CNode(self.cnode).mint(
                    fault.slot,
                    self.cnode,
                    fault.source,
                    RIGHTS_ALL,
                    fault.badge,
                )?;
                fault.slot
            }
            None => 0,
        };
        // SAFETY: unstarted thread; the shared pair is its execution context.
        unsafe { thread.set_space(self.cnode, self.vspace, fault_slot)? };
        // SAFETY: the frame is the one just mapped at `ipc`.
        unsafe { thread.set_ipc_buffer(CPtr(slot + 6), ipc)? };
        // SAFETY: entry/stack were mapped above; resume starts the thread.
        unsafe { thread.write_initial_registers(entry, stack_top, argument, true) }
    }
}

fn window(index: usize) -> usize {
    THREAD_WINDOW + index * THREAD_STRIDE
}
