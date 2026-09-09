//! Capability lookup and invocation. Object IDs never cross the user ABI.
mod cnode;
mod invoke;
mod runtime;
use crate::{
    memory::{AddressSpace, frame::Frame},
    task::api,
    utils::single_core::SingleCore,
};
use alloc::{collections::BTreeMap, rc::Rc, vec::Vec};
use core::cell::RefCell;
pub(crate) use invoke::call;
use kernel_abi::*;

const MAX_OBJECTS: usize = 4096;
const MAX_CAPS: usize = 8192;
const MAX_DERIVATIONS: usize = 16384;
type Space = Rc<RefCell<AddressSpace>>;
type Result<T> = core::result::Result<T, u64>;
#[derive(Clone)]
enum Object {
    Untyped,
    Tcb(u64),
    VSpace {
        owner: u64,
        space: Option<Space>,
        assigned: bool,
    },
    CNode,
    Page(Rc<Frame>),
    PageTable(Rc<Frame>),
    AsidPool,
    Runtime,
}
#[derive(Clone, Copy)]
struct Mapping {
    space: u64,
    address: usize,
    table: bool,
}
#[derive(Clone)]
struct Cap {
    serial: u64,
    object: u64,
    rights: u64,
    mapping: Option<Mapping>,
}
struct Store {
    objects: BTreeMap<u64, Object>,
    cspaces: BTreeMap<u64, BTreeMap<u64, Cap>>,
    parents: BTreeMap<u64, u64>,
    task_spaces: BTreeMap<u64, u64>,
    managed: alloc::collections::BTreeSet<u64>,
    next_object: u64,
    next_serial: u64,
}
static STORE: SingleCore<Store> = SingleCore::new(Store {
    objects: BTreeMap::new(),
    cspaces: BTreeMap::new(),
    parents: BTreeMap::new(),
    task_spaces: BTreeMap::new(),
    managed: alloc::collections::BTreeSet::new(),
    next_object: 1,
    next_serial: 1,
});
fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    f(&mut STORE.borrow_mut())
}
impl Store {
    fn object(&mut self, object: Object) -> u64 {
        let id = self.next_object;
        self.next_object = id.checked_add(1).expect("object identity exhausted");
        if matches!(object, Object::CNode) {
            self.cspaces.insert(id, BTreeMap::new());
        }
        self.objects.insert(id, object);
        id
    }
    fn cap(&self, cspace: u64, slot: u64) -> Result<Cap> {
        if slot == 0 || slot >= CNODE_SLOTS {
            return Err(NOT_FOUND);
        }
        self.cspaces
            .get(&cspace)
            .and_then(|s| s.get(&slot))
            .cloned()
            .ok_or(NOT_FOUND)
    }
    fn insert(
        &mut self,
        cspace: u64,
        slot: u64,
        object: u64,
        rights: u64,
        parent: u64,
    ) -> Result<()> {
        if slot == 0 || slot >= CNODE_SLOTS {
            return Err(RANGE_ERROR);
        }
        if self.cspaces.values().map(|s| s.len()).sum::<usize>() >= MAX_CAPS
            || self.parents.len() >= MAX_DERIVATIONS
        {
            return Err(NO_MEMORY);
        }
        let slots = self.cspaces.get_mut(&cspace).ok_or(INVALID_CAPABILITY)?;
        if slots.contains_key(&slot) {
            return Err(ALREADY_MAPPED);
        }
        let serial = self.next_serial;
        self.next_serial = serial.checked_add(1).ok_or(NO_MEMORY)?;
        self.parents.insert(serial, parent);
        slots.insert(
            slot,
            Cap {
                serial,
                object,
                rights,
                mapping: None,
            },
        );
        Ok(())
    }
    fn empty_slot(&self, cspace: u64) -> Result<u64> {
        let slots = self.cspaces.get(&cspace).ok_or(INVALID_CAPABILITY)?;
        (FIRST_FREE_SLOT..CNODE_SLOTS)
            .find(|s| !slots.contains_key(s))
            .ok_or(NO_MEMORY)
    }
    fn payload(&self, cap: &Cap) -> Result<Object> {
        self.objects
            .get(&cap.object)
            .cloned()
            .ok_or(INVALID_CAPABILITY)
    }
}
fn resolve(slot: u64) -> Result<(Cap, Object)> {
    let cspace = api::current_cspace();
    with_store(|s| {
        let cap = s.cap(cspace, slot)?;
        let object = s.payload(&cap)?;
        Ok((cap, object))
    })
}
fn tcb(slot: u64) -> Result<u64> {
    let (cap, object) = resolve(slot)?;
    if cap.rights & RIGHTS_WRITE == 0 {
        return Err(PERMISSION_DENIED);
    }
    match object {
        Object::Tcb(id) => Ok(id),
        _ => Err(INVALID_CAPABILITY),
    }
}
fn vspace(slot: u64) -> Result<(u64, Space)> {
    let (cap, object) = resolve(slot)?;
    if cap.rights & RIGHTS_WRITE == 0 {
        return Err(PERMISSION_DENIED);
    }
    match object {
        Object::VSpace {
            space: Some(space),
            assigned: true,
            ..
        } => Ok((cap.object, space)),
        _ => Err(INVALID_CAPABILITY),
    }
}
fn cnode(slot: u64) -> Result<u64> {
    let (cap, object) = resolve(slot)?;
    if cap.rights & RIGHTS_WRITE == 0 {
        return Err(PERMISSION_DENIED);
    }
    match object {
        Object::CNode => Ok(cap.object),
        _ => Err(INVALID_CAPABILITY),
    }
}

pub(crate) fn init_root(task: u64, space: Space, ipc: usize) -> u64 {
    with_store(|s| {
        let tcb = s.object(Object::Tcb(task));
        let vspace = s.object(Object::VSpace {
            owner: task,
            space: Some(space.clone()),
            assigned: true,
        });
        let cnode = s.object(Object::CNode);
        let untyped = s.object(Object::Untyped);
        let runtime = s.object(Object::Runtime);
        let asid = s.object(Object::AsidPool);
        let page = s.object(Object::Page(
            space.borrow().frame_at(ipc).expect("initial IPC frame"),
        ));
        for (slot, object) in [
            (INIT_TCB, tcb),
            (INIT_CNODE, cnode),
            (INIT_VSPACE, vspace),
            (INIT_UNTYPED, untyped),
            (INIT_RUNTIME, runtime),
            (INIT_ASID_POOL, asid),
            (INIT_IPC_BUFFER, page),
        ] {
            s.insert(cnode, slot, object, RIGHTS_ALL, 0).unwrap();
        }
        s.task_spaces.insert(task, cnode);
        s.managed.insert(task);
        cnode
    })
}

/// Managed tasks receive an isolated CSpace. Authority is represented by caps,
/// never inferred from a user's integer matching a global task ID.
fn publish_task(task: u64, space: Space, ipc: usize) -> Result<u64> {
    let parent = api::current_cspace();
    let (slot, child_cspace) = with_store(|s| {
        if s.objects.len() + 4 > MAX_OBJECTS
            || s.cspaces.values().map(|s| s.len()).sum::<usize>() + 8 > MAX_CAPS
            || s.parents.len() + 8 > MAX_DERIVATIONS
        {
            return Err(NO_MEMORY);
        }
        let slot = s.empty_slot(parent)?;
        let untyped = s.cap(parent, INIT_UNTYPED)?;
        let runtime = s.cap(parent, INIT_RUNTIME)?;
        let tcb = s.object(Object::Tcb(task));
        let vspace = s.object(Object::VSpace {
            owner: task,
            space: Some(space.clone()),
            assigned: true,
        });
        let cnode = s.object(Object::CNode);
        s.insert(parent, slot, tcb, RIGHTS_ALL, 0)?;
        let source = s.cap(parent, slot)?;
        s.insert(cnode, INIT_TCB, tcb, RIGHTS_ALL, source.serial)?;
        s.insert(cnode, INIT_CNODE, cnode, RIGHTS_ALL, 0)?;
        s.insert(cnode, INIT_VSPACE, vspace, RIGHTS_ALL, 0)?;
        s.insert(
            cnode,
            INIT_UNTYPED,
            untyped.object,
            untyped.rights,
            untyped.serial,
        )?;
        s.insert(
            cnode,
            INIT_RUNTIME,
            runtime.object,
            runtime.rights,
            runtime.serial,
        )?;
        let page = s.object(Object::Page(
            space.borrow().frame_at(ipc).map_err(|e| e as u64)?,
        ));
        s.insert(cnode, INIT_IPC_BUFFER, page, RIGHTS_ALL, 0)?;
        s.task_spaces.insert(task, cnode);
        s.managed.insert(task);
        Ok::<_, u64>((slot, cnode))
    })?;
    api::bind_cspace(task, child_cspace, ipc)?;
    Ok(slot)
}

/// Managed runtime policy releases a terminated task's private address space.
/// Explicit frame capabilities retain their independent ownership.
pub(crate) fn retire_task(task: u64) {
    with_store(|s| {
        if !s.managed.contains(&task) {
            for object in s.objects.values_mut() {
                if let Object::VSpace { owner, .. } = object {
                    if *owner == task {
                        *owner = 0;
                    }
                }
            }
            return;
        }
        if let Some(&space) = s.task_spaces.get(&task) {
            if let Some(slots) = s.cspaces.get_mut(&space) {
                slots.remove(&INIT_IPC_BUFFER);
            }
        }
        for object in s.objects.values_mut() {
            if let Object::VSpace { owner, space, .. } = object {
                if *owner == task {
                    *space = None;
                }
            }
        }
        let referenced: alloc::collections::BTreeSet<u64> = s
            .cspaces
            .values()
            .flat_map(|slots| slots.values().map(|cap| cap.object))
            .collect();
        s.objects
            .retain(|id, object| !matches!(object, Object::Page(_)) || referenced.contains(id));
    });
}

pub(crate) fn forget_task(task: u64) {
    with_store(|s| {
        let cspace = s.task_spaces.remove(&task);
        if s.managed.remove(&task) {
            if let Some(cspace) = cspace {
                s.cspaces.remove(&cspace);
                for slots in s.cspaces.values_mut() {
                    slots.retain(|_, cap| cap.object != cspace);
                }
            }
        }
    });
}
