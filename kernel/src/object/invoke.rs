use super::*;
use crate::{
    api::{Completion, Request},
    arch::kernel::thread::TrapFrame,
};

pub(crate) fn call(slot: u64, message: &Request) -> Result<Completion> {
    let (cap, object) = resolve(slot)?;
    if !matches!(object, Object::Page(_)) && cap.rights & RIGHTS_WRITE == 0 {
        return Err(PERMISSION_DENIED);
    }
    match object {
        Object::Runtime => runtime::invoke(message),
        Object::CNode => {
            let completion = cnode::invoke(cap.object, message)?;
            let current = crate::task::current_id().unwrap();
            if !with_store(|s| {
                s.objects
                    .values()
                    .any(|object| matches!(object,Object::Tcb(id) if *id == current))
            }) {
                return Ok(Completion::park(crate::task::Disposition::Exit(0)));
            }
            Ok(completion)
        }
        Object::Untyped if message.label == Invocation::UntypedRetype as u64 => {
            retype(&cap, message)
        }
        Object::Tcb(target) => tcb_invoke(target, message),
        Object::Page(frame) => page(slot, &cap, frame, message, false),
        Object::PageTable(frame) => page(slot, &cap, frame, message, true),
        Object::AsidPool if message.label == Invocation::ArmAsidPoolAssign as u64 => {
            message.require(0, 1)?;
            let (cap, object) = resolve(message.caps[0])?;
            match object {
                Object::VSpace {
                    assigned: false, ..
                } => with_store(|s| {
                    if let Object::VSpace { assigned, .. } = s.objects.get_mut(&cap.object).unwrap()
                    {
                        *assigned = true;
                    }
                }),
                _ => return Err(INVALID_CAPABILITY),
            }
            // Logical assignment is explicit; the UP backend flushes ASID 0.
            Ok(Completion::done(None))
        }
        _ => Err(UNSUPPORTED),
    }
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
    if ![
        ObjectType::Tcb as u64,
        ObjectType::CNode as u64,
        ObjectType::VSpace as u64,
        ObjectType::SmallPage as u64,
        ObjectType::PageTable as u64,
    ]
    .contains(&kind)
    {
        return Err(INVALID_ARGUMENT);
    }
    if a[1]
        != if kind == ObjectType::CNode as u64 {
            CNODE_BITS
        } else {
            0
        }
    {
        return Err(INVALID_ARGUMENT);
    }
    with_store(|s| {
        if s.objects.len() + count as usize > MAX_OBJECTS
            || s.cspaces.values().map(|s| s.len()).sum::<usize>() + count as usize > MAX_CAPS
            || s.parents.len() + count as usize > MAX_DERIVATIONS
        {
            return Err(NO_MEMORY);
        }
        if (a[4]..a[4] + count).any(|slot| s.cspaces[&node].contains_key(&slot)) {
            Err(ALREADY_MAPPED)
        } else {
            Ok(())
        }
    })?;
    let mut objects = Vec::new();
    let result = (|| {
        for _ in 0..count {
            let object = match kind {
                n if n == ObjectType::Tcb as u64 => Object::Tcb(api::create()?),
                n if n == ObjectType::CNode as u64 => Object::CNode,
                n if n == ObjectType::VSpace as u64 => Object::VSpace {
                    owner: 0,
                    assigned: false,
                    space: Some(Rc::new(RefCell::new(
                        AddressSpace::new().map_err(|e| e as u64)?,
                    ))),
                },
                n if n == ObjectType::SmallPage as u64 => {
                    Object::Page(Rc::new(Frame::allocate().map_err(|e| e as u64)?))
                }
                _ => Object::PageTable(Rc::new(Frame::allocate().map_err(|e| e as u64)?)),
            };
            objects.push(object);
        }
        Ok::<_, u64>(())
    })();
    if let Err(error) = result {
        for object in objects {
            if let Object::Tcb(id) = object {
                let _ = api::destroy(id);
            }
        }
        return Err(error);
    }
    with_store(|s| {
        for (index, object) in objects.into_iter().enumerate() {
            let object = s.object(object);
            s.insert(node, a[4] + index as u64, object, RIGHTS_ALL, cap.serial)
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
            if a[0] != 0 || ![0, 64 - CNODE_BITS].contains(&a[1]) || a[2] != 0 || a[3] & 1023 != 0 {
                return Err(INVALID_ARGUMENT);
            }
            let cspace = cnode(request.caps[0])?;
            let (space_id, space) = vspace(request.caps[1])?;
            if a[3] != 0 {
                let (_, object) = resolve(request.caps[2])?;
                let Object::Page(frame) = object else {
                    return Err(INVALID_CAPABILITY);
                };
                if space
                    .borrow()
                    .frame_at(a[3] as usize)
                    .map_err(|e| e as u64)?
                    .physical()
                    != frame.physical()
                {
                    return Err(INVALID_CAPABILITY);
                }
            }
            let owner = with_store(|s| match s.objects.get(&space_id) {
                Some(Object::VSpace { owner, .. }) => *owner,
                _ => 0,
            });
            if owner != 0 && owner != target {
                return Err(INVALID_CAPABILITY);
            }
            api::configure(target, cspace, space, a[3] as usize)?;
            with_store(|s| {
                s.task_spaces.insert(target, cspace);
            });
            with_store(|s| {
                // Reconfiguration must release the previous root's task reference.
                for object in s.objects.values_mut() {
                    if let Object::VSpace { owner, .. } = object {
                        if *owner == target {
                            *owner = 0;
                        }
                    }
                }
                if let Some(Object::VSpace { owner, .. }) = s.objects.get_mut(&space_id) {
                    *owner = target;
                }
            });
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
    frame: Rc<Frame>,
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
    if request.label == map {
        request.require(if table { 2 } else { 3 }, 1)?;
        let (space_id, space) = vspace(request.caps[0])?;
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
            .is_some_and(|m| m.space != space_id || m.address != address)
        {
            return Err(INVALID_ARGUMENT);
        }
        if table {
            // One physical page table must not back unrelated VSpaces or VA
            // regions: their software page ownership records are independent.
            let mapped = with_store(|s| {
                s.cspaces
                    .values()
                    .flat_map(|slots| slots.values())
                    .any(|other| other.object == cap.object && other.mapping.is_some())
            });
            if mapped {
                return Err(INVALID_ARGUMENT);
            }
            space
                .borrow_mut()
                .map_table(address, frame)
                .map_err(|e| e as u64)?;
        } else {
            let rights = request.words[1] & cap.rights;
            if rights & RIGHTS_READ == 0 {
                return Err(INVALID_ARGUMENT);
            }
            let permissions = 1
                | if rights & RIGHTS_WRITE != 0 { 2 } else { 0 }
                | if attr & VM_EXECUTE_NEVER == 0 { 4 } else { 0 };
            if cap.mapping.is_some() {
                if !space
                    .borrow()
                    .frame_at(address)
                    .is_ok_and(|mapped| Rc::ptr_eq(&mapped, &frame))
                {
                    return Err(INVALID_CAPABILITY);
                }
                space
                    .borrow_mut()
                    .protect(address, 4096, permissions)
                    .map_err(|e| e as u64)?;
            } else {
                space
                    .borrow_mut()
                    .map_page(address, frame, permissions)
                    .map_err(|e| e as u64)?;
            }
        }
        let cspace = api::current_cspace();
        with_store(|s| {
            s.cspaces
                .get_mut(&cspace)
                .unwrap()
                .get_mut(&slot)
                .unwrap()
                .mapping = Some(Mapping {
                space: space_id,
                address,
                table,
            })
        });
    } else if request.label == unmap {
        if let Some(mapping) = cap.mapping {
            let space = with_store(|s| match s.objects.get(&mapping.space) {
                Some(Object::VSpace { space, .. }) => space.clone(),
                _ => None,
            });
            if let Some(space) = space {
                if table {
                    space
                        .borrow_mut()
                        .unmap_table(mapping.address)
                        .map_err(|e| e as u64)?;
                } else if space
                    .borrow()
                    .frame_at(mapping.address)
                    .is_ok_and(|mapped| Rc::ptr_eq(&mapped, &frame))
                {
                    space
                        .borrow_mut()
                        .unmap(mapping.address, 4096)
                        .map_err(|e| e as u64)?;
                }
            }
            let cspace = api::current_cspace();
            with_store(|s| {
                s.cspaces
                    .get_mut(&cspace)
                    .unwrap()
                    .get_mut(&slot)
                    .unwrap()
                    .mapping = None
            });
        }
    } else if !table && request.label == Invocation::ArmPageGetAddress as u64 {
        return Ok(Completion::done(Some(frame.physical() as u64)));
    } else {
        return Err(UNSUPPORTED);
    }
    Ok(Completion::done(None))
}
