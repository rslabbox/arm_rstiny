//! Authorized task operations. Task-table representation stays inside this module's parent.
use super::*;
use crate::memory::{UserConstPtr, UserPtr};

fn actor(scheduler: &Scheduler) -> usize {
    scheduler
        .current_slot()
        .expect("operation outside user event")
}
fn authorized<T>(
    target: u64,
    operation: impl FnOnce(&mut Scheduler, usize, usize) -> Result<T, u64>,
) -> Result<T, u64> {
    with_scheduler(|scheduler| {
        let caller = actor(scheduler);
        let target = scheduler.lookup(caller, target)?;
        operation(scheduler, caller, target)
    })
}
fn space(scheduler: &Scheduler, slot: usize) -> Result<&AddressSpace, u64> {
    scheduler.tasks[slot].space.as_ref().ok_or(INVALID_STATE)
}
fn editable(scheduler: &Scheduler, caller: usize, target: usize) -> Result<(), u64> {
    if target != caller && !matches!(scheduler.tasks[target].state, TASK_CREATED | TASK_SUSPENDED) {
        return Err(BUSY);
    }
    Ok(())
}

pub(crate) fn create() -> Result<u64, u64> {
    let space = AddressSpace::new().map_err(|e| e as u64)?;
    with_scheduler(|scheduler| {
        let caller = actor(scheduler);
        let parent = scheduler.tasks[caller].id;
        let slot = scheduler.create(parent, space)?;
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
    authorized(target, |scheduler, _, slot| {
        if scheduler.tasks[slot].state != TASK_CREATED {
            return Err(INVALID_STATE);
        }
        if entry & 3 != 0 || stack & 15 != 0 {
            return Err(INVALID_ARGUMENT);
        }
        space(scheduler, slot)?
            .check(entry as usize, 4, 4)
            .map_err(|e| e as u64)?;
        let bottom = stack.checked_sub(16).ok_or(INVALID_ARGUMENT)?;
        space(scheduler, slot)?
            .check(bottom as usize, 16, 2)
            .map_err(|e| e as u64)?;
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
    authorized(target, |scheduler, _, slot| Ok(scheduler.tasks[slot].state))
}
pub(crate) fn suspend(target: u64) -> Result<(), u64> {
    authorized(target, |scheduler, _, slot| {
        let task = &mut scheduler.tasks[slot];
        if !task.started || task.terminal() || task.state == TASK_SUSPENDED {
            return Err(INVALID_STATE);
        }
        task.suspended_from = task.state;
        task.state = TASK_SUSPENDED;
        scheduler.queue.remove(slot);
        Ok(())
    })
}
pub(crate) fn resume(target: u64) -> Result<(), u64> {
    authorized(target, |scheduler, _, slot| {
        if scheduler.tasks[slot].state != TASK_SUSPENDED {
            return Err(INVALID_STATE);
        }
        match scheduler.tasks[slot].suspended_from {
            TASK_WAITING => scheduler.tasks[slot].state = TASK_WAITING,
            TASK_SLEEPING if irq::now() < scheduler.tasks[slot].deadline => {
                scheduler.tasks[slot].state = TASK_SLEEPING
            }
            _ => scheduler.ready(slot),
        }
        Ok(())
    })
}
pub(crate) fn destroy(target: u64) -> Result<(), u64> {
    authorized(target, |scheduler, caller, slot| {
        if slot == caller {
            return Err(INVALID_ARGUMENT);
        }
        if !scheduler.tasks[slot].terminal() {
            scheduler.finish(slot, false, u64::MAX);
        }
        scheduler.tasks[slot] = Task::empty();
        Ok(())
    })
}
/// None means the runtime must commit a wait; it is not a completed syscall.
pub(crate) fn wait_result(target: u64) -> Result<Option<u64>, u64> {
    authorized(target, |scheduler, caller, slot| {
        if caller == slot {
            return Err(INVALID_ARGUMENT);
        }
        Ok(scheduler.tasks[slot]
            .terminal()
            .then_some(scheduler.tasks[slot].result))
    })
}
/// Run an address-space operation after ownership and stopped-task checks.
/// The closure's result cannot borrow the space beyond the scheduler borrow.
pub(crate) fn edit_space<T>(
    target: u64,
    operation: impl FnOnce(&mut AddressSpace) -> Result<T, crate::memory::Error>,
) -> Result<T, u64> {
    authorized(target, |scheduler, caller, slot| {
        editable(scheduler, caller, slot)?;
        operation(scheduler.tasks[slot].space.as_mut().ok_or(INVALID_STATE)?).map_err(|e| e as u64)
    })
}
/// Stage the complete source before writing, including self-copy and overlaps.
fn copy_between(
    scheduler: &mut Scheduler,
    source_slot: usize,
    source: UserConstPtr<u8>,
    destination_slot: usize,
    destination: UserPtr<u8>,
    buffer: &mut [u8],
) -> Result<(), u64> {
    source
        .read(space(scheduler, source_slot)?, buffer)
        .map_err(|e| e as u64)?;
    destination
        .write(
            scheduler.tasks[destination_slot]
                .space
                .as_mut()
                .ok_or(INVALID_STATE)?,
            buffer,
        )
        .map_err(|e| e as u64)
}

/// Copy from the caller into an authorized, editable target.
pub(crate) fn write_memory(
    target: u64,
    destination: UserPtr<u8>,
    source: UserConstPtr<u8>,
    buffer: &mut [u8],
) -> Result<(), u64> {
    authorized(target, |scheduler, caller, slot| {
        editable(scheduler, caller, slot)?;
        copy_between(scheduler, caller, source, slot, destination, buffer)
    })
}

/// Copy from an authorized, editable target into the caller.
pub(crate) fn read_memory(
    target: u64,
    source: UserConstPtr<u8>,
    destination: UserPtr<u8>,
    buffer: &mut [u8],
) -> Result<(), u64> {
    authorized(target, |scheduler, caller, slot| {
        editable(scheduler, caller, slot)?;
        copy_between(scheduler, slot, source, caller, destination, buffer)
    })
}
