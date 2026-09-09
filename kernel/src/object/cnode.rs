//! Flat CSpace slots with derivation tracking, attenuation and revocation.
use super::*;
use crate::api::{Completion, Request};

fn descendants(store: &Store, serial: u64, ancestor: u64) -> bool {
    let mut current = serial;
    while let Some(&parent) = store.parents.get(&current) {
        if parent == ancestor {
            return true;
        }
        if parent == 0 {
            break;
        }
        current = parent;
    }
    false
}
fn unmap(cap: &Cap) -> Result<()> {
    if let Some(mapping) = cap.mapping {
        let space = with_store(|s| match s.objects.get(&mapping.space) {
            Some(Object::VSpace { space, .. }) => space.clone(),
            _ => None,
        });
        if let Some(space) = space {
            if mapping.table {
                space
                    .borrow_mut()
                    .unmap_table(mapping.address)
                    .map_err(|e| e as u64)?;
            } else {
                let frame = with_store(|s| match s.objects.get(&cap.object) {
                    Some(Object::Page(frame)) => frame.clone(),
                    _ => unreachable!(),
                });
                if space
                    .borrow()
                    .frame_at(mapping.address)
                    .is_ok_and(|mapped| Rc::ptr_eq(&mapped, &frame))
                {
                    space
                        .borrow_mut()
                        .unmap(mapping.address, crate::memory::PAGE_SIZE)
                        .map_err(|e| e as u64)?;
                }
            }
        }
    }
    Ok(())
}
pub(super) fn delete(cspace: u64, slot: u64, revoke: bool) -> Result<()> {
    let victims = with_store(|s| {
        let cap = s.cap(cspace, slot)?;
        let mut victims = Vec::new();
        if revoke {
            for (&space, slots) in &s.cspaces {
                for (&slot, child) in slots {
                    if descendants(s, child.serial, cap.serial) {
                        victims.push((space, slot, child.clone()));
                    }
                }
            }
        } else {
            victims.push((cspace, slot, cap));
        }
        Ok::<_, u64>(victims)
    })?;
    // Revoke page mappings before deleting the containing page-table caps.
    for table in [false, true] {
        for (space, slot, cap) in &victims {
            if cap.mapping.is_some_and(|m| m.table == table) {
                unmap(cap)?;
                // A later nonempty page table may reject deletion. Preserve
                // accurate cap state so retrying cannot unmap a reused VA.
                with_store(|s| {
                    s.cspaces
                        .get_mut(space)
                        .unwrap()
                        .get_mut(slot)
                        .unwrap()
                        .mapping = None;
                });
            }
        }
    }
    with_store(|s| {
        for (space, slot, _) in victims {
            if let Some(slots) = s.cspaces.get_mut(&space) {
                slots.remove(&slot);
            }
        }
    });
    collect();
    Ok(())
}
pub(super) fn collect() {
    loop {
        let (tasks, changed) = with_store(|s| {
            let live: alloc::collections::BTreeSet<u64> = s
                .cspaces
                .values()
                .flat_map(|slots| slots.values().map(|cap| cap.object))
                .collect();
            let dead: Vec<u64> = s
            .objects
            .keys()
            .filter(|id| {
                !live.contains(id) && !s.task_spaces.values().any(|node| node == *id) &&
                !matches!(s.objects.get(id), Some(Object::VSpace {owner,space:Some(_),..}) if *owner != 0)
            })
            .copied()
            .collect();
            let changed = !dead.is_empty();
            let mut tasks = Vec::new();
            for id in dead {
                match s.objects.remove(&id) {
                    Some(Object::Tcb(task)) => tasks.push(task),
                    Some(Object::CNode) => {
                        s.cspaces.remove(&id);
                    }
                    _ => {}
                }
            }
            // Keep deleted ancestors only while a live descendant still needs them.
            let mut needed = alloc::collections::BTreeSet::new();
            for cap in s.cspaces.values().flat_map(|slots| slots.values()) {
                let mut serial = cap.serial;
                while serial != 0 && needed.insert(serial) {
                    serial = s.parents.get(&serial).copied().unwrap_or(0);
                }
            }
            s.parents.retain(|serial, _| needed.contains(serial));
            (tasks, changed)
        });
        for task in tasks {
            // The managed runtime handles self-termination after switching stacks.
            if Some(task) != crate::task::current_id() {
                let _ = api::destroy(task);
            }
        }
        if !changed {
            break;
        }
    }
}
pub(super) fn invoke(cnode: u64, request: &Request) -> Result<Completion> {
    let a = &request.words;
    request.require(2, 0)?;
    if a[1] != 64 {
        return Err(NOT_FOUND);
    }
    match request.label {
        n if n == Invocation::CNodeDelete as u64 => delete(cnode, a[0], false)?,
        n if n == Invocation::CNodeRevoke as u64 => delete(cnode, a[0], true)?,
        n if n == Invocation::CNodeCopy as u64
            || n == Invocation::CNodeMint as u64
            || n == Invocation::CNodeMove as u64 =>
        {
            let moving = n == Invocation::CNodeMove as u64;
            let mint = n == Invocation::CNodeMint as u64;
            request.require(
                if moving {
                    4
                } else if mint {
                    6
                } else {
                    5
                },
                1,
            )?;
            if a[3] != 64 {
                return Err(NOT_FOUND);
            }
            // Badge/guard mutation is not part of this flat, unbadged subset.
            if mint && a[5] != 0 {
                return Err(INVALID_ARGUMENT);
            }
            let source = super::cnode(request.caps[0])?;
            with_store(|s| {
                let cap = s.cap(source, a[2])?;
                let rights = if !moving && matches!(s.payload(&cap)?, Object::Page(_)) {
                    cap.rights & a[4] & RIGHTS_ALL
                } else {
                    cap.rights
                };
                if moving {
                    if a[0] == 0 || a[0] >= CNODE_SLOTS {
                        return Err(RANGE_ERROR);
                    }
                    if s.cspaces[&cnode].contains_key(&a[0]) {
                        return Err(ALREADY_MAPPED);
                    }
                    s.cspaces.get_mut(&source).unwrap().remove(&a[2]);
                    s.cspaces.get_mut(&cnode).unwrap().insert(a[0], cap);
                    Ok(())
                } else {
                    s.insert(cnode, a[0], cap.object, rights, cap.serial)
                }
            })?;
        }
        _ => return Err(UNSUPPORTED),
    }
    Ok(Completion::done(None))
}
