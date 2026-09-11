//! ELF loading through Untyped, CNode, VSpace, PageTable, Page and TCB invocations.
use crate::{Error, Task, abi, capability::*, runtime};
use rstiny_elf::Elf;

const PAGE: usize = 4096;
const STACK_SIZE: usize = 64 * 1024;

fn empty_slot() -> Result<u64, Error> {
    runtime(abi::RuntimeInvocation::FindEmptySlot, &[])
}
fn retype(allocator: &Untyped, kind: ObjectType) -> Result<u64, Error> {
    let slot = empty_slot()?;
    allocator.retype(
        kind,
        if kind == ObjectType::CNode {
            CNODE_BITS
        } else {
            0
        },
        CPtr(INIT_CNODE),
        slot,
        1,
    )?;
    Ok(slot)
}
fn ensure_table_at(
    allocator: &Untyped,
    space: CPtr,
    address: usize,
    tables: &mut [bool; 64],
    slots: &mut impl FnMut() -> u64,
) -> Result<(), Error> {
    let index = address >> 21;
    if !tables[index] {
        let table = retype_at(allocator, ObjectType::PageTable, slots)?;
        PageTable(CPtr(table)).map(space, address & !(0x200000 - 1))?;
        tables[index] = true;
    }
    Ok(())
}

fn retype_at(
    allocator: &Untyped,
    kind: ObjectType,
    slots: &mut impl FnMut() -> u64,
) -> Result<u64, Error> {
    let slot = slots();
    allocator.retype(
        kind,
        if kind == ObjectType::CNode {
            CNODE_BITS
        } else {
            0
        },
        CPtr(INIT_CNODE),
        slot,
        1,
    )?;
    Ok(slot)
}

/// Load a static ELF with a private stack, IPC buffer and CSpace.
/// The root supplies an unused scratch VA according to its own address policy.
/// All created objects derive from a private copy of its allocator capability.
///
/// # Safety
/// `scratch` must be an unreferenced, page-aligned and currently unmapped page
/// in the caller's address space, exclusively reserved for this operation.
pub unsafe fn spawn(image: &[u8], scratch: usize, source_untyped: u64) -> Result<Task, Error> {
    let elf = Elf::parse(image).map_err(|_| Error::InvalidArgument)?;
    if scratch < PAGE
        || scratch >= abi::USER_ADDRESS_LIMIT as usize
        || scratch % PAGE != 0
        || elf.segments().any(|s| !matches!(s.flags, 4..=6))
    {
        return Err(Error::InvalidArgument);
    }
    let stack_bottom = elf.end().checked_add(PAGE).ok_or(Error::InvalidArgument)?;
    let stack_top = stack_bottom
        .checked_add(STACK_SIZE)
        .ok_or(Error::InvalidArgument)?;
    let ipc = stack_top;
    let pages: usize = elf.segments().map(|s| (s.end - s.va) / PAGE).sum();
    if elf.start() < PAGE
        || ipc + PAGE > abi::USER_ADDRESS_LIMIT as usize
        || pages + STACK_SIZE / PAGE + 1 > abi::MAX_USER_PAGES
    {
        return Err(Error::InvalidArgument);
    }

    let cnode = CNode(CPtr(INIT_CNODE));
    let mut next_slot = LOADER_SLOT_BASE;
    let mut loader_slot = || {
        let slot = next_slot;
        next_slot += 1;
        slot
    };
    let allocator_slot = loader_slot();
    cnode.copy(allocator_slot, CPtr(INIT_CNODE), source_untyped, RIGHTS_ALL)?;
    let allocator = Untyped(CPtr(allocator_slot));
    let _ = &cnode;
    let result = (|| {
        let tcb = retype_at(&allocator, ObjectType::Tcb, &mut loader_slot)?;
        let space = CPtr(retype(&allocator, ObjectType::VSpace)?);
        let child_node = CPtr(retype(&allocator, ObjectType::CNode)?);

        // Assign this root before any page-table/frame invocation can use it.
        unsafe {
            CPtr(INIT_ASID_POOL).call(Invocation::ArmAsidPoolAssign, &[], &[space.0])?;
        }
        let mut tables = [false; 64];
        let mut scratch_table = None;

        for segment in elf.segments() {
            for address in (segment.va..segment.end).step_by(PAGE) {
                ensure_table_at(&allocator, space, address, &mut tables, &mut loader_slot)?;
                let page_slot = retype_at(&allocator, ObjectType::SmallPage, &mut loader_slot)?;
                let page = Page(CPtr(page_slot));
                // A copied frame cap carries its own mapping state. Initialize
                // through a temporary RW/NX alias in the caller's VSpace.
                let alias_slot = loader_slot();
                cnode.copy(alias_slot, CPtr(INIT_CNODE), page_slot, RIGHTS_ALL)?;
                let alias = Page(CPtr(alias_slot));
                let map = unsafe {
                    alias.map(
                        CPtr(INIT_VSPACE),
                        scratch,
                        RIGHTS_READ | RIGHTS_WRITE,
                        VM_CACHEABLE | VM_EXECUTE_NEVER,
                    )
                };
                if map == Err(Error::FailedLookup) {
                    let table = retype(&allocator, ObjectType::PageTable)?;
                    PageTable(CPtr(table)).map(CPtr(INIT_VSPACE), scratch & !(0x200000 - 1))?;
                    scratch_table = Some(table);
                    unsafe {
                        alias.map(
                            CPtr(INIT_VSPACE),
                            scratch,
                            RIGHTS_READ | RIGHTS_WRITE,
                            VM_CACHEABLE | VM_EXECUTE_NEVER,
                        )?;
                    }
                } else {
                    map?;
                }
                let offset = address - segment.va;
                let count = segment.filesz.saturating_sub(offset).min(PAGE);
                if count != 0 {
                    // SAFETY: a newly allocated zeroed frame exclusively mapped
                    // into the caller's reserved scratch page; ELF validated input.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            image.as_ptr().add(segment.offset + offset),
                            scratch as *mut u8,
                            count,
                        );
                    }
                }
                unsafe {
                    alias.unmap()?;
                    cnode.delete(alias_slot)?;
                }
                let rights = RIGHTS_READ | if segment.flags == 6 { RIGHTS_WRITE } else { 0 };
                let attr = VM_CACHEABLE
                    | if segment.flags == 5 {
                        0
                    } else {
                        VM_EXECUTE_NEVER
                    };
                unsafe {
                    page.map(space, address, rights, attr)?;
                }
            }
        }
        for address in (stack_bottom..stack_top).step_by(PAGE) {
            ensure_table_at(&allocator, space, address, &mut tables, &mut loader_slot)?;
            let page = retype_at(&allocator, ObjectType::SmallPage, &mut loader_slot)?;
            unsafe {
                Page(CPtr(page)).map(
                    space,
                    address,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_CACHEABLE | VM_EXECUTE_NEVER,
                )?;
            }
        }
        ensure_table_at(&allocator, space, ipc, &mut tables, &mut loader_slot)?;
        let ipc_frame = retype_at(&allocator, ObjectType::SmallPage, &mut loader_slot)?;
        unsafe {
            Page(CPtr(ipc_frame)).map(
                space,
                ipc,
                RIGHTS_READ | RIGHTS_WRITE,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )?;
        }
        // Grant only self-management and runtime services. The child does not
        // receive the root allocator or the parent's CSpace.
        let child = CNode(child_node);
        for (slot, source) in [
            (INIT_TCB, tcb),
            (INIT_CNODE, child_node.0),
            (INIT_VSPACE, space.0),
            (abi::INIT_IPC_BUFFER, ipc_frame),
            (abi::INIT_RUNTIME, abi::INIT_RUNTIME),
        ] {
            child.copy(slot, CPtr(INIT_CNODE), source, RIGHTS_ALL)?;
        }
        if let Some(table) = scratch_table {
            unsafe {
                cnode.delete(table)?;
            }
        }
        let thread = Tcb(CPtr(tcb));
        unsafe {
            thread.configure(child_node, space, CPtr(ipc_frame), ipc, 0)?;
            thread.write_initial_registers(elf.entry(), stack_top, 0, false)?;
            thread.resume()?;
        }
        Ok(Task::from_objects(tcb, allocator_slot))
    })();
    if result.is_err() {
        // No other task owns capabilities derived from this private allocator.
        unsafe {
            cnode.revoke(allocator_slot)?;
            cnode.delete(allocator_slot)?;
        }
    }
    result
}

/// A capability installed into the child CNode before the child starts.
#[derive(Clone, Copy)]
pub struct ChildCap {
    /// Child CSpace slot.
    pub slot: u64,
    /// Caller CSpace slot of the source capability.
    pub source: u64,
    pub rights: u64,
    /// Nonzero mints an endpoint badge; zero copies the capability.
    pub badge: u64,
}

/// Supervision parameters for [`spawn_supervised`].
pub struct Supervision<'a> {
    /// Parameter page contents handed to the child as its start argument.
    pub info: &'a [u8],
    /// Child CSpace slot of the fault endpoint (0 = unmonitored).
    pub fault_ep: u64,
    /// Capabilities minted or copied into the child CNode.
    pub caps: &'a [ChildCap],
    /// First CSpace slot (in the *supervisor's* CSpace) this spawn allocates
    /// from. Concurrent spawns of sibling services need disjoint windows;
    /// [`LOADER_SLOT_BASE`] plus a per-service stride is the convention.
    pub slot_base: u64,
}

/// First CSpace slot the supervised loader allocates from by default. Well
/// below the supervisor's ChildCap window (see docs/service-manager.md) so
/// the two never collide; the loader climbs monotonically from here.
pub const LOADER_SLOT_BASE: u64 = 40_000;
/// Slot window reserved per supervised spawn, so sibling services loading
/// concurrently in one supervisor never overlap.
pub const LOADER_SLOT_STRIDE: u64 = 4_000;

/// Load a static ELF under a supervisor: the child receives a read-only
/// parameter page (its start argument), the listed capabilities and a fault
/// endpoint, all installed before its first instruction. Rollback on failure
/// revokes the private allocator, so a half-built child leaves nothing behind.
///
/// # Safety
/// `scratch` must be an unreferenced, page-aligned and currently unmapped page
/// in the caller's address space, exclusively reserved for this operation.
pub unsafe fn spawn_supervised(
    image: &[u8],
    scratch: usize,
    source_untyped: u64,
    spec: &Supervision<'_>,
) -> Result<Task, Error> {
    let elf = Elf::parse(image).map_err(|_| Error::InvalidArgument)?;
    if scratch < PAGE
        || scratch >= abi::USER_ADDRESS_LIMIT as usize
        || scratch % PAGE != 0
        || elf.segments().any(|s| !matches!(s.flags, 4..=6))
        || spec.info.len() > PAGE
    {
        return Err(Error::InvalidArgument);
    }
    let stack_bottom = elf.end().checked_add(PAGE).ok_or(Error::InvalidArgument)?;
    let stack_top = stack_bottom
        .checked_add(STACK_SIZE)
        .ok_or(Error::InvalidArgument)?;
    let ipc = stack_top;
    let info_va = ipc.checked_add(PAGE).ok_or(Error::InvalidArgument)?;
    let pages: usize = elf.segments().map(|s| (s.end - s.va) / PAGE).sum();
    if elf.start() < PAGE
        || info_va + PAGE > abi::USER_ADDRESS_LIMIT as usize
        || pages + STACK_SIZE / PAGE + 2 > abi::MAX_USER_PAGES
    {
        return Err(Error::InvalidArgument);
    }

    let cnode = CNode(CPtr(INIT_CNODE));
    let mut next_slot = spec.slot_base;
    let mut loader_slot = || {
        let slot = next_slot;
        next_slot += 1;
        slot
    };
    let allocator_slot = loader_slot();
    cnode.copy(allocator_slot, CPtr(INIT_CNODE), source_untyped, RIGHTS_ALL)?;
    let allocator = Untyped(CPtr(allocator_slot));
    let _ = &cnode;
    let result = (|| {
        let tcb = retype_at(&allocator, ObjectType::Tcb, &mut loader_slot)?;
        let space = CPtr(retype(&allocator, ObjectType::VSpace)?);
        let child_node = CPtr(retype(&allocator, ObjectType::CNode)?);

        // Assign this root before any page-table/frame invocation can use it.
        unsafe {
            CPtr(INIT_ASID_POOL).call(Invocation::ArmAsidPoolAssign, &[], &[space.0])?;
        }
        let mut tables = [false; 64];
        let mut scratch_table = None;

        for segment in elf.segments() {
            for address in (segment.va..segment.end).step_by(PAGE) {
                ensure_table_at(&allocator, space, address, &mut tables, &mut loader_slot)?;
                let page_slot = retype_at(&allocator, ObjectType::SmallPage, &mut loader_slot)?;
                let page = Page(CPtr(page_slot));
                // A copied frame cap carries its own mapping state. Initialize
                // through a temporary RW/NX alias in the caller's VSpace.
                let alias_slot = loader_slot();
                cnode.copy(alias_slot, CPtr(INIT_CNODE), page_slot, RIGHTS_ALL)?;
                let alias = Page(CPtr(alias_slot));
                let map = unsafe {
                    alias.map(
                        CPtr(INIT_VSPACE),
                        scratch,
                        RIGHTS_READ | RIGHTS_WRITE,
                        VM_CACHEABLE | VM_EXECUTE_NEVER,
                    )
                };
                if map == Err(Error::FailedLookup) {
                    let table = retype(&allocator, ObjectType::PageTable)?;
                    PageTable(CPtr(table)).map(CPtr(INIT_VSPACE), scratch & !(0x200000 - 1))?;
                    scratch_table = Some(table);
                    unsafe {
                        alias.map(
                            CPtr(INIT_VSPACE),
                            scratch,
                            RIGHTS_READ | RIGHTS_WRITE,
                            VM_CACHEABLE | VM_EXECUTE_NEVER,
                        )?;
                    }
                } else {
                    map?;
                }
                let offset = address - segment.va;
                let count = segment.filesz.saturating_sub(offset).min(PAGE);
                if count != 0 {
                    // SAFETY: a newly allocated zeroed frame exclusively mapped
                    // into the caller's reserved scratch page; ELF validated input.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            image.as_ptr().add(segment.offset + offset),
                            scratch as *mut u8,
                            count,
                        );
                    }
                }
                unsafe {
                    alias.unmap()?;
                    cnode.delete(alias_slot)?;
                }
                let rights = RIGHTS_READ | if segment.flags == 6 { RIGHTS_WRITE } else { 0 };
                let attr = VM_CACHEABLE
                    | if segment.flags == 5 {
                        0
                    } else {
                        VM_EXECUTE_NEVER
                    };
                unsafe {
                    page.map(space, address, rights, attr)?;
                }
            }
        }
        for address in (stack_bottom..stack_top).step_by(PAGE) {
            ensure_table_at(&allocator, space, address, &mut tables, &mut loader_slot)?;
            let page = retype_at(&allocator, ObjectType::SmallPage, &mut loader_slot)?;
            unsafe {
                Page(CPtr(page)).map(
                    space,
                    address,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_CACHEABLE | VM_EXECUTE_NEVER,
                )?;
            }
        }
        ensure_table_at(&allocator, space, ipc, &mut tables, &mut loader_slot)?;
        let ipc_frame = retype_at(&allocator, ObjectType::SmallPage, &mut loader_slot)?;
        unsafe {
            Page(CPtr(ipc_frame)).map(
                space,
                ipc,
                RIGHTS_READ | RIGHTS_WRITE,
                VM_CACHEABLE | VM_EXECUTE_NEVER,
            )?;
        }
        // The parameter page: filled through the scratch alias, then mapped
        // read-only in the child so only the supervisor can rewrite it.
        ensure_table_at(&allocator, space, info_va, &mut tables, &mut loader_slot)?;
        let info_frame = retype_at(&allocator, ObjectType::SmallPage, &mut loader_slot)?;
        {
            let alias_slot = loader_slot();
            cnode.copy(alias_slot, CPtr(INIT_CNODE), info_frame, RIGHTS_ALL)?;
            let alias = Page(CPtr(alias_slot));
            unsafe {
                alias.map(
                    CPtr(INIT_VSPACE),
                    scratch,
                    RIGHTS_READ | RIGHTS_WRITE,
                    VM_CACHEABLE | VM_EXECUTE_NEVER,
                )?;
                core::ptr::copy_nonoverlapping(
                    spec.info.as_ptr(),
                    scratch as *mut u8,
                    spec.info.len(),
                );
                core::ptr::write_bytes(
                    (scratch + spec.info.len()) as *mut u8,
                    0,
                    PAGE - spec.info.len(),
                );
                alias.unmap()?;
                cnode.delete(alias_slot)?;
            }
        }
        unsafe {
            Page(CPtr(info_frame)).map(space, info_va, RIGHTS_READ, VM_CACHEABLE)?;
        }
        // Grant only self-management and runtime services, then the
        // supervisor's capabilities, before the child starts.
        let child = CNode(child_node);
        for (slot, source) in [
            (INIT_TCB, tcb),
            (INIT_CNODE, child_node.0),
            (INIT_VSPACE, space.0),
            (abi::INIT_IPC_BUFFER, ipc_frame),
            (abi::INIT_RUNTIME, abi::INIT_RUNTIME),
        ] {
            child.copy(slot, CPtr(INIT_CNODE), source, RIGHTS_ALL)?;
        }
        for cap in spec.caps {
            if cap.badge != 0 {
                child.mint(
                    cap.slot,
                    CPtr(INIT_CNODE),
                    cap.source,
                    cap.rights,
                    cap.badge,
                )?;
            } else {
                child.copy(cap.slot, CPtr(INIT_CNODE), cap.source, cap.rights)?;
            }
        }
        if let Some(table) = scratch_table {
            unsafe {
                cnode.delete(table)?;
            }
        }
        let thread = Tcb(CPtr(tcb));
        unsafe {
            thread.configure(child_node, space, CPtr(ipc_frame), ipc, spec.fault_ep)?;
            thread.write_initial_registers(elf.entry(), stack_top, info_va as u64, false)?;
            thread.resume()?;
        }
        Ok(Task::from_objects(tcb, allocator_slot))
    })();
    if result.is_err() {
        // No other task owns capabilities derived from this private allocator.
        unsafe {
            cnode.revoke(allocator_slot)?;
            cnode.delete(allocator_slot)?;
        }
    }
    result
}
