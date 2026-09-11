use super::*;
use crate::{
    api::{Completion, Request},
    arch::kernel::thread::TrapFrame,
};

/// Object invocation entry point. Collection runs after the whole operation so
/// that multi-step creations are reachable before reclamation examines them.
pub(crate) fn call(slot: u64, message: &Request) -> Result<Completion> {
    let result = dispatch(slot, message);
    super::collect_if_requested();
    result
}

fn dispatch(slot: u64, message: &Request) -> Result<Completion> {
    let (cap, kind) = resolve(slot)?;
    // IPC capabilities are dispatched at the syscall layer and never reach
    // here; frame mappings carry their own rights.
    if !matches!(
        kind,
        ObjectKind::Frame | ObjectKind::PageTable | ObjectKind::Endpoint | ObjectKind::Notification
    ) && cap.rights & RIGHTS_WRITE == 0
    {
        log::info!(
            "DBG denied slot={} kind={:?} rights={:#x}",
            slot,
            kind,
            cap.rights
        );
        return Err(PERMISSION_DENIED);
    }
    match kind {
        ObjectKind::Runtime => runtime::invoke(message),
        ObjectKind::IrqControl if message.label == Invocation::IrqIssueIrqHandler as u64 => {
            irq::issue(&cap, message)
        }
        ObjectKind::IrqHandler => irq::handler_invoke(&cap, message),
        ObjectKind::CNode => {
            let completion = cnode::invoke(cap.object, message)?;
            let current = crate::task::current_id().unwrap();
            if !with_store(|store| {
                store
                    .objects
                    .iter()
                    .any(|(_, object)| matches!(object, Object::Tcb(id) if *id == current))
            }) {
                return Ok(Completion::park(crate::task::Disposition::Exit(0)));
            }
            Ok(completion)
        }
        ObjectKind::Untyped if message.label == Invocation::UntypedRetype as u64 => {
            retype(&cap, message)
        }
        ObjectKind::Tcb => {
            let target = tcb_object(cap.object)?;
            tcb_invoke(target, message)
        }
        ObjectKind::Frame => {
            let frame = with_store(|store| store.frame_ref(cap.object))?;
            page(slot, &cap, frame, message, false)
        }
        ObjectKind::PageTable => {
            let frame = with_store(|store| store.frame_ref(cap.object))?;
            page(slot, &cap, frame, message, true)
        }
        ObjectKind::AsidPool if message.label == Invocation::ArmAsidPoolAssign as u64 => {
            message.require(0, 1)?;
            let (target, kind) = resolve(message.caps[0])?;
            if kind != ObjectKind::VSpace {
                return Err(INVALID_CAPABILITY);
            }
            with_store(|store| match store.objects.get_mut(target.object) {
                Some(Object::VSpace(vspace)) if !vspace.assigned => {
                    vspace.assigned = true;
                    Ok(())
                }
                _ => Err(INVALID_CAPABILITY),
            })?;
            // Logical assignment is explicit; the UP backend flushes ASID 0.
            Ok(Completion::done(None))
        }
        ObjectKind::VSpace if message.label == Invocation::ArmVspaceTranslate as u64 => {
            // Self-translation only: a task may resolve virtual to physical
            // for its own address space (DMA setup), never for another task's.
            message.require(1, 0)?;
            let current = crate::task::current_id().ok_or(INVALID_CAPABILITY)?;
            if crate::task::api::vspace_of(current)? != cap.object {
                return Err(PERMISSION_DENIED);
            }
            let address = message.words[0] as usize;
            let physical = crate::object::edit_vspace(cap.object, |space| {
                space
                    .translate(memory_addr::VirtAddr::from_usize(address))
                    .map(|translation| translation.physical.as_usize())
            })?;
            Ok(Completion::done(Some(physical as u64)))
        }
        _ => Err(UNSUPPORTED),
    }
}

fn tcb_object(id: ObjectId) -> Result<u64> {
    with_store(|store| match store.objects.get(id) {
        Some(Object::Tcb(task)) => Ok(*task),
        _ => Err(INVALID_CAPABILITY),
    })
}

fn retype(cap: &Cap, request: &Request) -> Result<Completion> {
    request.require(6, 1)?;
    let a = &request.words;
    if a[2] != 0 || a[3] != 0 {
        return Err(NOT_FOUND);
    }
    let node = cnode(request.caps[0])?;
    let count = a[5];
    if count == 0
        || count > 32
        || a[4] == 0
        || a[4].checked_add(count).is_none_or(|end| end > CNODE_SLOTS)
    {
        return Err(RANGE_ERROR);
    }
    let kind = a[0];
    let child_untyped = kind == ObjectType::Untyped as u64;
    if !child_untyped
        && ![
            ObjectType::Tcb as u64,
            ObjectType::CNode as u64,
            ObjectType::VSpace as u64,
            ObjectType::SmallPage as u64,
            ObjectType::PageTable as u64,
            ObjectType::Endpoint as u64,
            ObjectType::Notification as u64,
        ]
        .contains(&kind)
    {
        return Err(INVALID_ARGUMENT);
    }
    let (is_device, parent_size_bits) = with_store(|store| match store.objects.get(cap.object) {
        Some(Object::Untyped(untyped)) => Ok((untyped.is_device(), untyped.size_bits())),
        _ => Err(INVALID_CAPABILITY),
    })?;
    // Device memory may only become device frames.
    if is_device && kind != ObjectType::SmallPage as u64 {
        return Err(INVALID_ARGUMENT);
    }
    // A child Untyped is an aligned sub-region of the parent: service budgets
    // are carved this way, and `Revoke` on the child cap resets only the child.
    let (object_bytes, object_align) = if child_untyped {
        if a[1] < untyped::MIN_SIZE_BITS as u64
            || a[1] > (untyped::MAX_SIZE_BITS as u64).min(parent_size_bits as u64)
        {
            return Err(INVALID_ARGUMENT);
        }
        (1usize << a[1], 1usize << a[1])
    } else {
        if a[1]
            != if kind == ObjectType::CNode as u64 {
                CNODE_BITS
            } else {
                0
            }
        {
            return Err(INVALID_ARGUMENT);
        }
        // Every VSpace currently carries three private page-table pages; TCB
        // and CNode are billed their nominal metadata size even though the
        // payload lives in the object table.
        object_allocation(kind, a[1]).ok_or(INVALID_ARGUMENT)?
    };
    let extra = if kind == ObjectType::VSpace as u64 {
        3 * count as usize
    } else {
        0
    };
    with_store(|store| {
        if store.objects.len() + count as usize + extra > MAX_OBJECTS
            || store.caps + count as usize > MAX_CAPS
            || store.parents.len() + count as usize > MAX_DERIVATIONS
        {
            return Err(NO_MEMORY);
        }
        let occupied = {
            let cnode = store.cnode(node)?;
            (a[4]..a[4] + count).any(|slot| cnode.slots.contains_key(&(slot as u16)))
        };
        if occupied {
            return Err(ALREADY_MAPPED);
        }
        // Exact watermark simulation: alignment padding is accounted per object.
        if !store
            .untyped(cap.object)?
            .fits(count as usize, object_bytes, object_align)
        {
            return Err(NO_MEMORY);
        }
        Ok(())
    })?;
    // Scheduler tasks are created before the object-table borrow and destroyed
    // on any later failure.
    let mut tasks = Vec::new();
    if kind == ObjectType::Tcb as u64 {
        for _ in 0..count {
            match api::create() {
                Ok(task) => tasks.push(task),
                Err(error) => {
                    for task in tasks {
                        let _ = api::destroy(task);
                    }
                    return Err(error);
                }
            }
        }
    }
    with_store(|store| {
        let mut staged = Vec::new();
        for index in 0..count {
            let id = match kind {
                n if n == ObjectType::Tcb as u64 => {
                    let (_, offset) = store
                        .untyped_reserve(cap.object, object_bytes, object_align)
                        .expect("reserved budget");
                    let owner = ObjectOwner {
                        untyped: cap.object,
                        offset,
                        size: object_bytes,
                    };
                    store
                        .objects
                        .insert_owned(Object::Tcb(tasks[index as usize]), Some(owner))
                        .expect("reserved capacity")
                }
                n if n == ObjectType::CNode as u64 => {
                    let (_, offset) = store
                        .untyped_reserve(cap.object, object_bytes, object_align)
                        .expect("reserved budget");
                    let owner = ObjectOwner {
                        untyped: cap.object,
                        offset,
                        size: object_bytes,
                    };
                    store
                        .objects
                        .insert_owned(Object::CNode(CNode::new()), Some(owner))
                        .expect("reserved capacity")
                }
                n if n == ObjectType::VSpace as u64 => store
                    .new_vspace(Some(cap.object))
                    .expect("reserved capacity"),
                n if n == ObjectType::SmallPage as u64 => store
                    .new_untyped_frame(cap.object, false)
                    .expect("reserved capacity")
                    .id(),
                n if n == ObjectType::Endpoint as u64 || n == ObjectType::Notification as u64 => {
                    let (_, offset) = store
                        .untyped_reserve(cap.object, object_bytes, object_align)
                        .expect("reserved budget");
                    let owner = ObjectOwner {
                        untyped: cap.object,
                        offset,
                        size: object_bytes,
                    };
                    let payload = if n == ObjectType::Endpoint as u64 {
                        Object::Endpoint(Endpoint::new())
                    } else {
                        Object::Notification(Notification::new())
                    };
                    store
                        .objects
                        .insert_owned(payload, Some(owner))
                        .expect("reserved capacity")
                }
                n if n == ObjectType::Untyped as u64 => {
                    let (physical, offset) = store
                        .untyped_allocate(cap.object, object_bytes, object_align)
                        .expect("reserved budget");
                    let device = match store.objects.get(cap.object) {
                        Some(Object::Untyped(region)) => region.is_device(),
                        _ => false,
                    };
                    let owner = ObjectOwner {
                        untyped: cap.object,
                        offset,
                        size: object_bytes,
                    };
                    store
                        .objects
                        .insert_owned(
                            Object::Untyped(Untyped::new(physical, a[1] as u8, device)),
                            Some(owner),
                        )
                        .expect("reserved capacity")
                }
                _ => store
                    .new_untyped_frame(cap.object, true)
                    .expect("reserved capacity")
                    .id(),
            };
            staged.push(id);
        }
        for (index, object) in staged.iter().enumerate() {
            store
                .insert_cap(
                    node,
                    a[4] + index as u64,
                    *object,
                    RIGHTS_ALL,
                    cap.serial,
                    0,
                )
                .expect("validated destination");
        }
    });
    Ok(Completion::done(None))
}

fn tcb_invoke(target: u64, request: &Request) -> Result<Completion> {
    let a = &request.words;
    match request.label {
        n if n == Invocation::TcbSuspend as u64 => {
            if api::status(target)? != TASK_CREATED {
                api::suspend(target)?;
            }
        }
        n if n == Invocation::TcbResume as u64 => api::resume(target)?,
        n if n == Invocation::TcbConfigure as u64 => {
            request.require(4, 3)?;
            // `a[0]` is the fault-endpoint slot, resolved in this task's own
            // CSpace when a fault is delivered (seL4 non-MCS semantics).
            if ![0, 64 - CNODE_BITS].contains(&a[1]) || a[2] != 0 || a[3] & 1023 != 0 {
                return Err(INVALID_ARGUMENT);
            }
            let cspace = cnode(request.caps[0])?;
            let space = vspace(request.caps[1])?;
            if a[3] != 0 {
                let (frame, kind) = resolve(request.caps[2])?;
                if kind != ObjectKind::Frame {
                    return Err(INVALID_CAPABILITY);
                }
                if vspace_frame_at(space, a[3] as usize)?.id() != frame.object {
                    return Err(INVALID_CAPABILITY);
                }
            }
            // The CSpace/VSpace pair may already be in use by other threads: a
            // thread group shares both, so no ownership check applies
            // (docs/fault-handler.md §3).
            let root = vspace_root(space)?;
            api::configure(target, cspace, space, root, a[3] as usize, a[0])?;
        }
        n if n == Invocation::TcbSetSpace as u64 => {
            request.require(3, 2)?;
            // seL4 `TCB_SetSpace`: fault-endpoint slot plus the CSpace/VSpace
            // roots, without touching the IPC buffer. Like `Tcb_Configure`
            // this is restricted to threads that never ran (v1 choice,
            // docs/fault-handler.md §12.3).
            if a[1] != 0 || a[2] != 0 {
                return Err(INVALID_ARGUMENT);
            }
            let cspace = cnode(request.caps[0])?;
            let space = vspace(request.caps[1])?;
            let root = vspace_root(space)?;
            api::set_space(target, cspace, space, root, a[0])?;
        }
        n if n == Invocation::TcbSetIpcBuffer as u64 => {
            request.require(1, 1)?;
            // seL4 `TCB_SetIPCBuffer`: the frame must already be mapped at the
            // given address in the thread's VSpace. Address 0 clears the
            // buffer; the frame capability is ignored in that case.
            let address = a[0] as usize;
            let mut frame = None;
            if address != 0 {
                if address & 1023 != 0 {
                    return Err(INVALID_ARGUMENT);
                }
                let (resolved, kind) = resolve(request.caps[0])?;
                if kind != ObjectKind::Frame {
                    return Err(INVALID_CAPABILITY);
                }
                frame = Some(resolved.object);
            }
            api::set_ipc_buffer(target, frame, address)?;
        }
        n if n == Invocation::TcbWriteRegisters as u64 => {
            request.require(2, 0)?;
            let count = a[1] as usize;
            if a[0] > 1 || !(4..=36).contains(&count) {
                return Err(INVALID_ARGUMENT);
            }
            request.require(count + 2, 0)?;
            let registers = &a[2..2 + count];
            let mut frame = TrapFrame::user(registers[0], registers[1], registers[3]);
            frame.spsr = registers[2];
            const ORDER: [usize; 31] = [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 16, 17, 18, 29, 30, 9, 10, 11, 12, 13, 14, 15, 19, 20,
                21, 22, 23, 24, 25, 26, 27, 28,
            ];
            for (index, &reg) in ORDER.iter().enumerate() {
                if index + 3 < count {
                    frame.r[reg] = registers[index + 3];
                }
            }
            if registers
                .get(34..)
                .is_some_and(|tls| tls.iter().any(|&v| v != 0))
            {
                return Err(INVALID_ARGUMENT);
            }
            api::write_registers(target, frame, a[0] != 0, crate::api::dispatch)?;
        }
        _ => return Err(UNSUPPORTED),
    }
    Ok(Completion::done(None))
}

fn page(
    slot: u64,
    cap: &Cap,
    frame: FrameRef,
    request: &Request,
    table: bool,
) -> Result<Completion> {
    let map = if table {
        Invocation::ArmPageTableMap
    } else {
        Invocation::ArmPageMap
    } as u64;
    let unmap = if table {
        Invocation::ArmPageTableUnmap
    } else {
        Invocation::ArmPageUnmap
    } as u64;
    let cspace = api::current_cspace();
    if request.label == map {
        request.require(if table { 2 } else { 3 }, 1)?;
        let space = vspace(request.caps[0])?;
        let address = request.words[0] as usize;
        let attr = request.words[if table { 1 } else { 2 }];
        if attr & !(VM_CACHEABLE | VM_EXECUTE_NEVER) != 0 {
            return Err(INVALID_ARGUMENT);
        }
        if address & 4095 != 0 {
            return Err(ALIGNMENT_ERROR);
        }
        if cap
            .mapping
            .is_some_and(|m| m.space != space || m.address != address)
        {
            return Err(INVALID_ARGUMENT);
        }
        if table {
            // One physical page table must not back unrelated VSpaces or VA
            // regions: their software page ownership records are independent.
            let mapped = with_store(|store| {
                store.objects.iter().any(|(_, object)| match object {
                    Object::CNode(cnode) => cnode
                        .slots
                        .values()
                        .any(|other| other.object == cap.object && other.mapping.is_some()),
                    _ => false,
                })
            });
            if mapped {
                return Err(INVALID_ARGUMENT);
            }
            with_store(|store| {
                store
                    .vspace_mut(space)?
                    .map_table(address, frame)
                    .map_err(|e| e as u64)
            })?;
        } else {
            let rights = request.words[1] & cap.rights;
            if rights & RIGHTS_READ == 0 {
                return Err(INVALID_ARGUMENT);
            }
            let permissions = 1
                | if rights & RIGHTS_WRITE != 0 { 2 } else { 0 }
                | if attr & VM_EXECUTE_NEVER == 0 { 4 } else { 0 };
            with_store(|store| {
                let space = store.vspace_mut(space)?;
                if cap.mapping.is_some() {
                    if space.frame_at(address).ok() != Some(frame) {
                        return Err(INVALID_CAPABILITY);
                    }
                    space
                        .protect(address, 4096, permissions)
                        .map_err(|e| e as u64)?;
                } else {
                    space
                        .map_page(address, frame, permissions)
                        .map_err(|e| e as u64)?;
                }
                Ok(())
            })?;
        }
        with_store(|store| {
            store
                .cnode_mut(cspace)?
                .slots
                .get_mut(&(slot as u16))
                .expect("resolved capability")
                .mapping = Some(Mapping {
                space,
                address,
                table,
            });
            Ok::<_, u64>(())
        })?;
    } else if request.label == unmap {
        if let Some(mapping) = cap.mapping {
            with_store(|store| {
                if let Some(space) = store.vspace_opt_mut(mapping.space) {
                    if table {
                        space.unmap_table(mapping.address).map_err(|e| e as u64)?;
                    } else if space.frame_at(mapping.address).ok() == Some(frame) {
                        space
                            .unmap(mapping.address, crate::memory::PAGE_SIZE)
                            .map_err(|e| e as u64)?;
                    }
                }
                if let Some(stored) = store
                    .cnode_mut(cspace)
                    .ok()
                    .and_then(|cnode| cnode.slots.get_mut(&(slot as u16)))
                {
                    stored.mapping = None;
                }
                Ok::<_, u64>(())
            })?;
            request_collect();
        }
    } else if !table && request.label == Invocation::ArmPageGetAddress as u64 {
        return Ok(Completion::done(Some(frame.physical() as u64)));
    } else {
        return Err(UNSUPPORTED);
    }
    Ok(Completion::done(None))
}
