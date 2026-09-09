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

/// Remove the mapping recorded on a capability. The frame object stays owned
/// by the object table; it becomes collectable once the capability is gone.
fn unmap(store: &mut Store, cap: &Cap) -> Result<()> {
    let Some(mapping) = cap.mapping else {
        return Ok(());
    };
    let frame = store.frame_ref(cap.object).ok();
    if let Some(space) = store.vspace_opt_mut(mapping.space) {
        if mapping.table {
            space.unmap_table(mapping.address).map_err(|e| e as u64)?;
        } else if let Some(frame) = frame
            && space.frame_at(mapping.address) == Ok(frame)
        {
            space
                .unmap(mapping.address, crate::memory::PAGE_SIZE)
                .map_err(|e| e as u64)?;
        }
    }
    Ok(())
}

/// Finalise every object carved from an Untyped region, then reset and clear it.
/// Capabilities naming the children must already have been removed. Child
/// Untyped regions are finalised depth-first, so nested service budgets tear
/// down completely; endpoints and notifications cancel their waiters first.
fn finalise_untyped(store: &mut Store, untyped: ObjectId) {
    for child in store.objects.children(untyped) {
        match store.objects.get(child) {
            Some(Object::Untyped(_)) => finalise_untyped(store, child),
            Some(Object::Endpoint(_)) | Some(Object::Notification(_)) => {
                api::suspend_blocked_on(child);
            }
            _ => {}
        }
        store.objects.remove(child);
    }
    if let Some(Object::Untyped(region)) = store.objects.get_mut(untyped) {
        region.reset();
        region.clear();
    }
}

pub(super) fn delete(cspace: ObjectId, slot: u64, revoke: bool) -> Result<()> {
    let (target, victims) = with_store(|store| {
        let cap = store.cap(cspace, slot)?;
        let mut victims = Vec::new();
        if revoke {
            for (space, object) in store.objects.iter() {
                if let Object::CNode(cnode) = object {
                    for (&other_slot, child) in &cnode.slots {
                        if descendants(store, child.serial, cap.serial) {
                            victims.push((space, other_slot as u64, child.clone()));
                        }
                    }
                }
            }
        } else {
            victims.push((cspace, slot, cap.clone()));
        }
        Ok::<_, u64>((cap.object, victims))
    })?;
    // Revoke page mappings before deleting the containing page-table caps.
    for table in [false, true] {
        for (space, slot, cap) in &victims {
            if cap.mapping.is_some_and(|m| m.table == table) {
                with_store(|store| unmap(store, cap))?;
                // A later nonempty page table may reject deletion. Preserve
                // accurate cap state so retrying cannot unmap a reused VA.
                with_store(|store| {
                    if let Some(stored) = store
                        .cnode_mut(*space)
                        .ok()
                        .and_then(|cnode| cnode.slots.get_mut(&(*slot as u16)))
                    {
                        stored.mapping = None;
                    }
                });
            }
        }
    }
    with_store(|store| {
        for (space, slot, _) in &victims {
            store.remove_cap(*space, *slot);
        }
        if revoke && matches!(store.objects.get(target), Some(Object::Untyped(_))) {
            finalise_untyped(store, target);
        }
    });
    super::request_collect();
    Ok(())
}

pub(super) fn invoke(cspace: ObjectId, request: &Request) -> Result<Completion> {
    let a = &request.words;
    request.require(2, 0)?;
    if a[1] != 64 {
        return Err(NOT_FOUND);
    }
    match request.label {
        n if n == Invocation::CNodeDelete as u64 => delete(cspace, a[0], false)?,
        n if n == Invocation::CNodeRevoke as u64 => delete(cspace, a[0], true)?,
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
            let source = super::cnode(request.caps[0])?;
            with_store(|store| {
                let cap = store.cap(source, a[2])?;
                if moving {
                    store.move_cap(source, a[2], cspace, a[0])
                } else {
                    // seL4 attenuation: the requested rights mask applies to
                    // every capability kind, not only frames.
                    let rights = cap.rights & a[4] & RIGHTS_ALL;
                    // seL4 badge semantics: only endpoint-style caps carry a
                    // badge, minting one requires Grant, and the result is
                    // AND-ed with the source badge.
                    let mut badge = cap.badge;
                    if mint && a[5] != 0 {
                        if !matches!(
                            store.kind(cap.object)?,
                            ObjectKind::Endpoint | ObjectKind::Notification
                        ) {
                            return Err(INVALID_ARGUMENT);
                        }
                        if cap.rights & RIGHTS_GRANT == 0 {
                            return Err(PERMISSION_DENIED);
                        }
                        if cap.badge != 0 {
                            return Err(UNSUPPORTED);
                        }
                        badge = a[5];
                    }
                    store.insert_cap(cspace, a[0], cap.object, rights, cap.serial, badge)
                }
            })?;
        }
        _ => return Err(UNSUPPORTED),
    }
    Ok(Completion::done(None))
}
