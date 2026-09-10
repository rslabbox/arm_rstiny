//! RSTiny managed-runtime services. These labels are deliberately not seL4 methods.
use super::*;
use crate::{
    api::{Completion, Request},
    arch::machine::time,
    memory::UserPtr,
    task::Disposition,
};
use RuntimeInvocation as R;

pub(super) fn invoke(request: &Request) -> Result<Completion> {
    let a = &request.words;
    let label = request.label;
    let words = match label {
        n if [
            R::Current as u64,
            R::Create as u64,
            R::Clock as u64,
            R::AvailableFrames as u64,
            R::FindEmptySlot as u64,
            R::DebugConsoleAvailable as u64,
        ]
        .contains(&n) =>
        {
            0
        }
        n if [
            R::Status as u64,
            R::Cspace as u64,
            R::Vspace as u64,
            R::Destroy as u64,
            R::DestroyThread as u64,
            R::Wait as u64,
            R::Sleep as u64,
            R::Exit as u64,
        ]
        .contains(&n) =>
        {
            1
        }
        n if n == R::Unmap as u64 => 3,
        n if [
            R::Start as u64,
            R::Map as u64,
            R::Protect as u64,
            R::WriteMemory as u64,
            R::ReadMemory as u64,
        ]
        .contains(&n) =>
        {
            4
        }
        _ => return Err(UNSUPPORTED),
    };
    request.require(words, 0)?;
    let value = match label {
        n if n == R::Current as u64 => {
            let current = crate::task::current_id().unwrap();
            let cspace = api::current_cspace();
            Some(with_store(|store| {
                store
                    .cnode(cspace)?
                    .slots
                    .iter()
                    .find_map(|(&slot, cap)| {
                        matches!(store.objects.get(cap.object), Some(Object::Tcb(id)) if *id == current)
                            .then_some(slot as u64)
                    })
                    .ok_or(NOT_FOUND)
            })?)
        }
        0x1010 => Some(u64::from(log::max_level() != log::LevelFilter::Off)),
        n if n == R::FindEmptySlot as u64 => {
            let cspace = api::current_cspace();
            Some(with_store(|store| store.empty_slot(cspace))?)
        }
        n if n == R::Create as u64 => {
            let task = api::create()?;
            let ipc = kernel_abi::USER_ADDRESS_LIMIT as usize - 4096;
            let result = (|| {
                let space = create_vspace()?;
                map_vspace(space, ipc, 4096, 3, true)?;
                publish_task(task, space, ipc)
            })();
            if result.is_err() {
                let _ = api::destroy(task);
            }
            Some(result?)
        }
        n if n == R::Start as u64 => {
            api::start(tcb(a[0])?, a[1], a[2], a[3], crate::api::dispatch)?;
            None
        }
        n if n == R::Cspace as u64 || n == R::Vspace as u64 => {
            let target = tcb(a[0])?;
            let object = if n == R::Cspace as u64 {
                api::cspace_of(target)?
            } else {
                api::vspace_of(target)?
            };
            let caller = api::current_cspace();
            Some(with_store(|store| {
                let slot = store.empty_slot(caller)?;
                store.insert_cap(caller, slot, object, RIGHTS_ALL, 0, 0)?;
                Ok::<_, u64>(slot)
            })?)
        }
        n if n == R::Status as u64 => Some(api::status(tcb(a[0])?)?),
        n if n == R::Destroy as u64 => {
            let target = tcb(a[0])?;
            if Some(target) == crate::task::current_id() {
                return Err(INVALID_ARGUMENT);
            }
            // Group semantics: the handle names a process (a shared CSpace),
            // so every member thread stops before any shared object is
            // released (docs/thread-group.md §2.2, §2.3).
            let members = api::group_members(target)?;
            if members.contains(&crate::task::current_id().unwrap()) {
                return Err(INVALID_ARGUMENT);
            }
            for member in members {
                // Captured before destruction clears the thread's bindings; a
                // VSpace shared with a surviving member is kept by the release
                // and only the last member's release retires it.
                let vspace = api::vspace_of(member).ok();
                api::destroy(member)?;
                release_task_objects(member, vspace);
            }
            None
        }
        n if n == R::DestroyThread as u64 => {
            let target = tcb(a[0])?;
            if Some(target) == crate::task::current_id() {
                return Err(INVALID_ARGUMENT);
            }
            // Single-thread teardown: shared CSpace/VSpace stay with the
            // surviving siblings (forget_task/release_task_objects check).
            let vspace = api::vspace_of(target).ok();
            api::destroy(target)?;
            release_task_objects(target, vspace);
            None
        }
        n if n == R::Wait as u64 => {
            let target = tcb(a[0])?;
            Some(match api::wait_result(target)? {
                Some(result) => result,
                None => crate::task::park(Disposition::Wait(target)).expect("wait completion"),
            })
        }
        n if n == R::Sleep as u64 => {
            let ticks = a[0]
                .checked_mul(time::frequency())
                .ok_or(INVALID_ARGUMENT)?
                / 1000;
            return Ok(Completion::park(Disposition::Sleep(
                time::now().checked_add(ticks).ok_or(INVALID_ARGUMENT)?,
            )));
        }
        n if n == R::Exit as u64 => return Ok(Completion::park(Disposition::Exit(a[0]))),
        n if n == R::Clock as u64 => Some(time::now() / (time::frequency() / 1000).max(1)),
        n if n == R::AvailableFrames as u64 => {
            Some((super::available_untyped() / crate::memory::PAGE_SIZE) as u64)
        }
        n if n == R::Map as u64 => {
            let space = api::editable_vspace(tcb(a[0])?)?;
            map_vspace(space, a[1] as usize, a[2] as usize, a[3], false)?;
            None
        }
        n if n == R::Unmap as u64 => {
            api::edit_space(tcb(a[0])?, |vspace| {
                vspace.unmap(a[1] as usize, a[2] as usize)
            })?;
            request_collect();
            None
        }
        n if n == R::Protect as u64 => {
            api::edit_space(tcb(a[0])?, |vspace| {
                vspace.protect(a[1] as usize, a[2] as usize, a[3])
            })?;
            None
        }
        n if n == R::WriteMemory as u64 || n == R::ReadMemory as u64 => {
            let len = a[3] as usize;
            if len > 4096 {
                return Err(INVALID_ARGUMENT);
            }
            let mut buffer = [0; 4096];
            let target = tcb(a[0])?;
            if n == R::WriteMemory as u64 {
                api::write_memory(target, UserPtr::from(a[1]), a[2].into(), &mut buffer[..len])?;
            } else {
                api::read_memory(target, a[1].into(), a[2].into(), &mut buffer[..len])?;
            }
            None
        }
        _ => return Err(UNSUPPORTED),
    };
    Ok(Completion::done(value))
}
