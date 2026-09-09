//! Internal task operations after capability authorization by the object layer.
use super::*;
use crate::arch::machine::time;
use crate::memory::{AddressSpace, UserConstPtr, UserPtr};
use crate::object::ObjectId;

fn actor(scheduler: &Scheduler) -> usize {
    scheduler
        .current_slot()
        .expect("operation outside user event")
}
fn with_target<T>(
    target: u64,
    operation: impl FnOnce(&mut Scheduler, usize, usize) -> Result<T, u64>,
) -> Result<T, u64> {
    with_scheduler(|scheduler| {
        let caller = actor(scheduler);
        let target = scheduler.lookup(target)?;
        operation(scheduler, caller, target)
    })
}
fn editable(scheduler: &Scheduler, caller: usize, target: usize) -> Result<(), u64> {
    // A thread blocked on its fault endpoint belongs to its supervisor: the
    // repair path (map, protect, write registers) runs while it waits.
    if target != caller
        && !matches!(
            scheduler.tasks[target].state,
            TASK_CREATED | TASK_SUSPENDED | TASK_BLOCKED_FAULT
        )
    {
        return Err(BUSY);
    }
    Ok(())
}

pub(crate) fn create() -> Result<u64, u64> {
    with_scheduler(|scheduler| {
        let caller = actor(scheduler);
        let parent = scheduler.tasks[caller].id;
        let slot = scheduler.create(parent)?;
        Ok(scheduler.tasks[slot].id)
    })
}
pub(crate) fn start(
    target: u64,
    entry: u64,
    stack: u64,
    argument: u64,
    dispatch: impl FnMut(&mut UserContext) -> Disposition + Send + 'static,
) -> Result<(), u64> {
    let vspace = vspace_of(target)?;
    with_target(target, |scheduler, _, slot| {
        if scheduler.tasks[slot].state != TASK_CREATED {
            return Err(INVALID_STATE);
        }
        if entry & 3 != 0 || stack & 15 != 0 {
            return Err(INVALID_ARGUMENT);
        }
        crate::object::with_vspace(vspace, |space| space.check(entry as usize, 4, 4))?;
        let bottom = stack.checked_sub(16).ok_or(INVALID_ARGUMENT)?;
        crate::object::with_vspace(vspace, |space| space.check(bottom as usize, 16, 2))?;
        let execution = new_user_task(
            UserContext::new(TrapFrame::user(entry, stack, argument)),
            dispatch,
        )
        .map_err(|error| error as u64)?;
        scheduler.tasks[slot].execution = Some(execution);
        scheduler.tasks[slot].started = true;
        scheduler.ready(slot);
        Ok(())
    })
}
pub(crate) fn status(target: u64) -> Result<u64, u64> {
    with_target(target, |scheduler, _, slot| Ok(scheduler.tasks[slot].state))
}
pub(crate) fn suspend(target: u64) -> Result<(), u64> {
    with_target(target, |scheduler, _, slot| {
        let task = &mut scheduler.tasks[slot];
        if !task.started || task.terminal() || task.state == TASK_SUSPENDED {
            return Err(INVALID_STATE);
        }
        task.suspended_from = task.state;
        task.state = TASK_SUSPENDED;
        // Queued entries are pruned by validation on the next touch.
        task.blocked = None;
        scheduler.queue.remove(slot);
        Ok(())
    })
}
pub(crate) fn resume(target: u64) -> Result<(), u64> {
    with_target(target, |scheduler, _, slot| {
        if scheduler.tasks[slot].state == TASK_CREATED && scheduler.tasks[slot].execution.is_some()
        {
            scheduler.tasks[slot].started = true;
            scheduler.ready(slot);
            return Ok(());
        }
        if scheduler.tasks[slot].state != TASK_SUSPENDED {
            return Err(INVALID_STATE);
        }
        match scheduler.tasks[slot].suspended_from {
            TASK_WAITING => scheduler.tasks[slot].state = TASK_WAITING,
            TASK_SLEEPING if time::now() < scheduler.tasks[slot].deadline => {
                scheduler.tasks[slot].state = TASK_SLEEPING
            }
            _ => scheduler.ready(slot),
        }
        Ok(())
    })
}
pub(crate) fn destroy(target: u64) -> Result<(), u64> {
    with_target(target, |scheduler, caller, slot| {
        if slot == caller {
            return Err(INVALID_ARGUMENT);
        }
        if !scheduler.tasks[slot].terminal() {
            scheduler.finish(slot, false, u64::MAX);
        }
        scheduler.tasks[slot] = Task::empty();
        crate::object::forget_task(target);
        Ok(())
    })
}
/// None means the runtime must commit a wait; it is not a completed syscall.
pub(crate) fn wait_result(target: u64) -> Result<Option<u64>, u64> {
    with_target(target, |scheduler, caller, slot| {
        if caller == slot {
            return Err(INVALID_ARGUMENT);
        }
        Ok(scheduler.tasks[slot]
            .terminal()
            .then_some(scheduler.tasks[slot].result))
    })
}
/// The VSpace object bound to a task. Every address-space operation resolves
/// through this identity; the scheduler never owns page tables.
pub(crate) fn vspace_of(target: u64) -> Result<ObjectId, u64> {
    with_target(target, |scheduler, _, slot| {
        scheduler.tasks[slot].vspace.ok_or(INVALID_STATE)
    })
}
/// Resolve the VSpace of a task only when the caller may edit it (self or a
/// created/suspended task). Mapping allocates object memory, so the caller
/// cannot hold a `&mut AddressSpace`; this returns the identity instead.
pub(crate) fn editable_vspace(target: u64) -> Result<ObjectId, u64> {
    with_target(target, |scheduler, caller, slot| {
        editable(scheduler, caller, slot)?;
        scheduler.tasks[slot].vspace.ok_or(INVALID_STATE)
    })
}
/// Run an address-space operation after ownership and stopped-task checks.
/// The closure's result cannot borrow the space beyond the store borrow.
pub(crate) fn edit_space<T>(
    target: u64,
    operation: impl FnOnce(&mut AddressSpace) -> Result<T, crate::memory::Error>,
) -> Result<T, u64> {
    with_target(target, |scheduler, caller, slot| {
        editable(scheduler, caller, slot)?;
        let vspace = scheduler.tasks[slot].vspace.ok_or(INVALID_STATE)?;
        crate::object::edit_vspace(vspace, operation)
    })
}
/// Every VSpace bound to a live task. Collection uses this as a root set so an
/// address space and its mapped frames stay reachable while a task holds them.
pub(crate) fn vspace_roots() -> alloc::vec::Vec<ObjectId> {
    with_scheduler(|scheduler| {
        scheduler
            .tasks
            .iter()
            .filter_map(|task| task.vspace)
            .collect()
    })
}

/// Copy from the caller into an authorized, editable target.
pub(crate) fn write_memory(
    target: u64,
    destination: UserPtr<u8>,
    source: UserConstPtr<u8>,
    buffer: &mut [u8],
) -> Result<(), u64> {
    let (caller, target) = with_target(target, |scheduler, caller, slot| {
        editable(scheduler, caller, slot)?;
        Ok((
            scheduler.tasks[caller].vspace.ok_or(INVALID_STATE)?,
            scheduler.tasks[slot].vspace.ok_or(INVALID_STATE)?,
        ))
    })?;
    crate::object::with_vspace(caller, |space| source.read(space, buffer))?;
    crate::object::edit_vspace(target, |space| destination.write(space, buffer))
}

/// Copy from an authorized, editable target into the caller.
pub(crate) fn read_memory(
    target: u64,
    source: UserConstPtr<u8>,
    destination: UserPtr<u8>,
    buffer: &mut [u8],
) -> Result<(), u64> {
    let (caller, target) = with_target(target, |scheduler, caller, slot| {
        editable(scheduler, caller, slot)?;
        Ok((
            scheduler.tasks[caller].vspace.ok_or(INVALID_STATE)?,
            scheduler.tasks[slot].vspace.ok_or(INVALID_STATE)?,
        ))
    })?;
    crate::object::with_vspace(target, |space| source.read(space, buffer))?;
    crate::object::edit_vspace(caller, |space| destination.write(space, buffer))
}

pub(crate) fn current_cspace() -> ObjectId {
    with_scheduler(|s| s.tasks[actor(s)].cspace.expect("current task CSpace"))
}
pub(crate) fn ipc_read(offset: usize, bytes: &mut [u8]) -> Result<(), u64> {
    let (vspace, address) = with_scheduler(|s| {
        let slot = actor(s);
        if s.tasks[slot].ipc_buffer == 0 {
            return Err(TRUNCATED_MESSAGE);
        }
        let address = s.tasks[slot]
            .ipc_buffer
            .checked_add(offset)
            .ok_or(INVALID_ARGUMENT)?;
        Ok((s.tasks[slot].vspace.ok_or(INVALID_STATE)?, address))
    })?;
    crate::object::with_vspace(vspace, |space| space.read(address, bytes))
}

/// Identity-addressed access for IPC delivery and fault handling. Unlike the
/// `with_target` helpers these need no running caller: cross-task wakeups and
/// collection run between tasks.
pub(super) fn with_task<T>(target: u64, operation: impl FnOnce(&mut Task) -> T) -> Result<T, u64> {
    with_scheduler(|scheduler| {
        let slot = scheduler.lookup(target)?;
        Ok(operation(&mut scheduler.tasks[slot]))
    })
}
/// Like [`with_task`] for operations that can themselves fail.
pub(super) fn try_task<T>(
    target: u64,
    operation: impl FnOnce(&mut Task) -> Result<T, u64>,
) -> Result<T, u64> {
    with_scheduler(|scheduler| {
        let slot = scheduler.lookup(target)?;
        operation(&mut scheduler.tasks[slot])
    })
}

/// Make a specific task schedulable; used by IPC delivery, never by policy.
pub(crate) fn wake(target: u64) -> Result<(), u64> {
    with_scheduler(|scheduler| {
        let slot = scheduler.lookup(target)?;
        scheduler.ready(slot);
        Ok(())
    })
}

/// Read a task's IPC buffer at `offset` (buffer-relative, as in `ipc_read`).
pub(crate) fn read_task_ipc(target: u64, offset: usize, bytes: &mut [u8]) -> Result<(), u64> {
    let (vspace, address) = try_task(target, |task| {
        if task.ipc_buffer == 0 {
            return Err(TRUNCATED_MESSAGE);
        }
        let address = task
            .ipc_buffer
            .checked_add(offset)
            .ok_or(INVALID_ARGUMENT)?;
        Ok((task.vspace.ok_or(INVALID_STATE)?, address))
    })?;
    crate::object::with_vspace(vspace, |space| space.read(address, bytes))
}

/// Write a task's IPC buffer at `offset`. One call covers one contiguous
/// region, so validation happens before the first byte is copied.
pub(crate) fn write_task_ipc(target: u64, offset: usize, bytes: &[u8]) -> Result<(), u64> {
    let (vspace, address) = try_task(target, |task| {
        if task.ipc_buffer == 0 {
            return Err(TRUNCATED_MESSAGE);
        }
        let address = task
            .ipc_buffer
            .checked_add(offset)
            .ok_or(INVALID_ARGUMENT)?;
        Ok((task.vspace.ok_or(INVALID_STATE)?, address))
    })?;
    crate::object::edit_vspace(vspace, |space| space.write(address, bytes))
}

/// The saved user frame of a non-running task: IPC delivery and fault repair.
pub(crate) fn edit_frame<T>(
    target: u64,
    operation: impl FnOnce(&mut crate::arch::kernel::thread::user::UserContext) -> T,
) -> Result<T, u64> {
    with_scheduler(|scheduler| {
        let slot = scheduler.lookup(target)?;
        let execution = scheduler.tasks[slot]
            .execution
            .as_mut()
            .ok_or(INVALID_STATE)?;
        Ok(operation(execution.frame_mut()))
    })
}

/// Suspend every task parked on `ep` because the object is going away. Runs
/// while the object store is borrowed, so stale queue entries are left for
/// the lazy validation in `peek_valid` to prune; a suspended continuation
/// re-enters its wait when a supervisor resumes it.
pub(crate) fn suspend_blocked_on(ep: ObjectId) {
    with_scheduler(|scheduler| {
        for task in scheduler.tasks.iter_mut() {
            let Some(blocked) = task.blocked else {
                continue;
            };
            if blocked.ep == ep && task.queued() {
                task.suspended_from = task.state;
                task.state = TASK_SUSPENDED;
                task.blocked = None;
            }
        }
    });
}

// IPC state accessors. The fields live on the scheduler's `Task`; the syscall
// layer only reads and writes them through these names.

pub(crate) fn state_of(target: u64) -> Result<u64, u64> {
    with_task(target, |task| task.state)
}
pub(crate) fn fault_endpoint(target: u64) -> u64 {
    with_task(target, |task| task.fault_ep).unwrap_or(0)
}
pub(crate) fn cspace_of(target: u64) -> Result<ObjectId, u64> {
    with_task(target, |task| task.cspace.ok_or(INVALID_STATE))?
}
pub(crate) fn blocked_of(target: u64) -> Option<super::Blocked> {
    with_task(target, |task| task.blocked).ok()?
}
pub(crate) fn set_blocked(target: u64, blocked: super::Blocked, state: u64) {
    with_task(target, |task| {
        task.blocked = Some(blocked);
        task.state = state;
    })
    .expect("blocked task identity");
}
/// Delivery or cancellation removes the wait: the continuation re-blocks.
pub(crate) fn clear_blocked(target: u64) {
    let _ = with_task(target, |task| task.blocked = None);
}
pub(crate) fn take_caller(target: u64) -> Option<super::Caller> {
    with_task(target, |task| task.caller.take()).ok()?
}
/// Returns the displaced relation, if any, so the caller can fail it.
pub(crate) fn set_caller(target: u64, caller: super::Caller) -> Option<super::Caller> {
    with_task(target, |task| task.caller.replace(caller)).ok()?
}
pub(crate) fn restart_pc_of(target: u64) -> u64 {
    with_task(target, |task| task.restart_pc).unwrap_or(0)
}
pub(crate) fn fault_msg_of(target: u64) -> Option<super::FaultMsg> {
    with_task(target, |task| task.fault_msg).ok()?
}
pub(crate) fn suspended_from_of(target: u64) -> u64 {
    with_task(target, |task| task.suspended_from).unwrap_or(TASK_CREATED)
}
/// Writability check of a task's IPC buffer region, without writing it.
pub(crate) fn check_task_ipc(target: u64, offset: usize, len: usize) -> Result<(), u64> {
    let (vspace, address) = try_task(target, |task| {
        if task.ipc_buffer == 0 {
            return Err(TRUNCATED_MESSAGE);
        }
        let address = task
            .ipc_buffer
            .checked_add(offset)
            .ok_or(INVALID_ARGUMENT)?;
        Ok((task.vspace.ok_or(INVALID_STATE)?, address))
    })?;
    crate::object::with_vspace(vspace, |space| space.check(address, len, 2))
}
/// Record a fault delivery context on the faulted task itself.
pub(crate) fn begin_fault(target: u64, ep: ObjectId, badge: u64, msg: super::FaultMsg, pc: u64) {
    with_task(target, |task| {
        task.restart_pc = pc;
        task.fault_msg = Some(msg);
        task.blocked = Some(super::Blocked {
            ep,
            badge,
            call: false,
            grant_reply: false,
            fault: true,
        });
        task.state = TASK_BLOCKED_FAULT;
    })
    .expect("faulting task identity");
}
/// Completion `Some(1)` resumes a blocked continuation with success; the
/// registers or restart PC were written by the waker beforehand.
pub(crate) fn complete(target: u64) -> Result<(), u64> {
    with_task(target, |task| {
        task.blocked = None;
        task.completion = Some(1);
    })?;
    wake(target)
}
/// Repark a delivered call sender: it now waits for this receiver's reply.
pub(crate) fn set_state(target: u64, state: u64) {
    let _ = with_task(target, |task| task.state = state);
}
/// Fail a relation displaced by a newer delivery, so its waiter never hangs.
pub(crate) fn fail_caller(displaced: super::Caller) {
    let target = displaced.task();
    let waiting = with_task(target, |task| {
        matches!(task.state, TASK_BLOCKED_REPLY | TASK_BLOCKED_FAULT)
            || (task.state == TASK_SUSPENDED
                && matches!(task.suspended_from, TASK_BLOCKED_REPLY | TASK_BLOCKED_FAULT))
    })
    .unwrap_or(false);
    if waiting {
        let _ = with_task(target, |task| task.completion = Some(0));
        let _ = wake(target);
    }
}
pub(crate) fn configure(
    target: u64,
    cspace: ObjectId,
    vspace: ObjectId,
    root: usize,
    ipc_buffer: usize,
    fault_ep: u64,
) -> Result<(), u64> {
    with_target(target, |s, _, slot| {
        if s.tasks[slot].started {
            return Err(INVALID_STATE);
        }
        if ipc_buffer != 0 {
            crate::object::with_vspace(vspace, |space| space.check(ipc_buffer, 1024, 3))?;
        }
        s.tasks[slot].root = root;
        s.tasks[slot].vspace = Some(vspace);
        s.tasks[slot].cspace = Some(cspace);
        s.tasks[slot].ipc_buffer = ipc_buffer;
        s.tasks[slot].fault_ep = fault_ep;
        Ok(())
    })
}
pub(crate) fn write_registers(
    target: u64,
    mut frame: TrapFrame,
    resume: bool,
    dispatch: impl FnMut(&mut UserContext) -> Disposition + Send + 'static,
) -> Result<(), u64> {
    let vspace = vspace_of(target)?;
    if frame.elr & 3 != 0
        || frame.usp & 15 != 0
        || frame.spsr & !0xf000_0000 != 0 && frame.spsr & !0xf000_0000 != 0x340
    {
        return Err(INVALID_ARGUMENT);
    }
    crate::object::with_vspace(vspace, |space| space.check(frame.elr as usize, 4, 4))?;
    let bottom = frame.usp.checked_sub(16).ok_or(INVALID_ARGUMENT)?;
    crate::object::with_vspace(vspace, |space| space.check(bottom as usize, 16, 2))?;
    frame.spsr = (frame.spsr & 0xf000_0000) | 0x340;
    // Fault repair: a supervisor rewrites the registers of a thread blocked on
    // its fault endpoint. The thread keeps its execution; only the saved user
    // frame changes, and `Reply` (not `resume`) returns it to EL0.
    if with_task(target, |task| task.state == TASK_BLOCKED_FAULT)? {
        if resume {
            return Err(INVALID_STATE);
        }
        return edit_frame(target, |saved| *saved = UserContext::new(frame));
    }
    with_target(target, |s, _, slot| {
        if s.tasks[slot].started {
            return Err(INVALID_STATE);
        }
        s.tasks[slot].execution =
            Some(new_user_task(UserContext::new(frame), dispatch).map_err(|e| e as u64)?);
        if resume {
            s.tasks[slot].started = true;
            s.ready(slot);
        }
        Ok(())
    })
}
