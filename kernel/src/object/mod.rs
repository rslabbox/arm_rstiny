//! Capability objects, CSpace lookup and object lifecycle.
//!
//! Ownership model (seL4 direction): [`ObjectTable`] is the single owner of
//! every kernel object payload. Capabilities, address spaces and tasks only
//! hold [`ObjectId`] handles. Physical frames are never shared through `Rc`;
//! an object is reclaimed only when collection determines that no capability
//! or live kernel binding reaches it, or when revocation removes the last
//! reference. Object IDs never cross the user ABI.
mod cnode;
mod endpoint;
mod id;
mod invoke;
pub(crate) mod irq;
mod runtime;
mod untyped;
use crate::{
    memory::{
        AddressSpace, Error as MemoryError, PAGE_SIZE,
        frame::{Frame, FrameRef},
    },
    task::api,
    utils::single_core::SingleCore,
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
pub(crate) use endpoint::{Endpoint, Notification, WaitQueue};
pub(crate) use id::{MAX_OBJECTS, ObjectId, ObjectOwner, ObjectTable};
pub(crate) use invoke::call;
use kernel_abi::*;
use untyped::Untyped;
pub(crate) use untyped::partition;

/// Nominal Untyped consumption of endpoint-style metadata objects. Like TCB
/// and CNode, the payload lives in the object table; only the budget is real.
pub(crate) const ENDPOINT_BYTES: usize = 64;
pub(crate) const ENDPOINT_ALIGN: usize = 64;

const MAX_CAPS: usize = 8192;
const MAX_DERIVATIONS: usize = 16384;
type Result<T> = core::result::Result<T, u64>;

/// Nominal Untyped consumption for kernel-metadata objects. Their payload
/// still lives in the object table, but the physical budget must be meaningful
/// and `MAX_OBJECTS` must stay a pure metadata cap.
pub(crate) const TCB_BYTES: usize = 1024;
pub(crate) const TCB_ALIGN: usize = 1024;
pub(crate) const CNODE_SLOT_BYTES: usize = 8;

/// `(bytes, align)` carved from an Untyped region for one object of `kind`.
/// `Untyped` is excluded: its size comes from the request's `size_bits`.
pub(crate) fn object_allocation(kind: u64, size_bits: u64) -> Option<(usize, usize)> {
    match kind {
        n if n == ObjectType::SmallPage as u64 || n == ObjectType::PageTable as u64 => {
            Some((PAGE_SIZE, PAGE_SIZE))
        }
        n if n == ObjectType::VSpace as u64 => Some((3 * PAGE_SIZE, PAGE_SIZE)),
        n if n == ObjectType::Tcb as u64 => Some((TCB_BYTES, TCB_ALIGN)),
        n if n == ObjectType::CNode as u64 => {
            let bytes = (1usize << size_bits).checked_mul(CNODE_SLOT_BYTES)?;
            Some((bytes, PAGE_SIZE))
        }
        n if n == ObjectType::Endpoint as u64 || n == ObjectType::Notification as u64 => {
            Some((ENDPOINT_BYTES, ENDPOINT_ALIGN))
        }
        _ => None,
    }
}

/// Typed view of an object payload, cheap to copy during capability dispatch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ObjectKind {
    Untyped,
    Tcb,
    VSpace,
    CNode,
    Frame,
    PageTable,
    AsidPool,
    Runtime,
    Endpoint,
    Notification,
    IrqControl,
    IrqHandler,
}

/// A recorded mapping installed by a frame or page-table capability. The
/// mapping lives and dies with the capability that created it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mapping {
    space: ObjectId,
    address: usize,
    table: bool,
}
#[derive(Clone, Debug)]
struct Cap {
    serial: u64,
    object: ObjectId,
    rights: u64,
    /// Endpoint/Notification badge delivered with messages. Only `CNode_Mint`
    /// changes it, by AND-ing with the source badge.
    badge: u64,
    mapping: Option<Mapping>,
}

/// Capability node: a bounded sparse table of capabilities. Its slots are part
/// of the object payload, so the object table owns them like any other object.
pub(crate) struct CNode {
    slots: BTreeMap<u16, Cap>,
}
impl CNode {
    const fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
        }
    }
    /// Test-only empty CNode; production ones come from retype or `init_root`.
    #[cfg(feature = "kernel-test")]
    pub(crate) const fn empty() -> Self {
        Self::new()
    }
}

/// VSpace payload: the address space plus its ASID binding. The address space
/// references page frames by identity; it never owns them. A VSpace is not
/// tied to one thread: any number of TCBs (a thread group) may reference the
/// same object, and collection keeps it alive until the last TCB or
/// capability reference disappears.
pub(crate) struct VSpace {
    assigned: bool,
    space: Option<AddressSpace>,
}

pub(crate) enum Object {
    Untyped(Untyped),
    Tcb(u64),
    VSpace(VSpace),
    CNode(CNode),
    Frame(Frame),
    PageTable(Frame),
    AsidPool,
    Runtime,
    Endpoint(Endpoint),
    Notification(Notification),
    /// Root-only IRQ authorization singleton (`INIT_IRQ_CONTROL`).
    IrqControl,
    /// One authorized interrupt line; created only by `IRQControl_Get`
    /// (docs/irq.md §3).
    IrqHandler(irq::IrqHandler),
}
impl Object {
    fn kind(&self) -> ObjectKind {
        match self {
            Object::Untyped(_) => ObjectKind::Untyped,
            Object::Tcb(_) => ObjectKind::Tcb,
            Object::VSpace(_) => ObjectKind::VSpace,
            Object::CNode(_) => ObjectKind::CNode,
            Object::Frame(_) => ObjectKind::Frame,
            Object::PageTable(_) => ObjectKind::PageTable,
            Object::AsidPool => ObjectKind::AsidPool,
            Object::Runtime => ObjectKind::Runtime,
            Object::Endpoint(_) => ObjectKind::Endpoint,
            Object::Notification(_) => ObjectKind::Notification,
            Object::IrqControl => ObjectKind::IrqControl,
            Object::IrqHandler(_) => ObjectKind::IrqHandler,
        }
    }
}

pub(crate) struct Store {
    objects: ObjectTable,
    parents: BTreeMap<u64, u64>,
    managed: BTreeSet<u64>,
    next_serial: u64,
    caps: usize,
    /// Untyped objects created by the boot partitioner, in BootInfo order.
    boot_untyped: Vec<ObjectId>,
    /// Physical pages of the boot-module archive, in ascending order.
    boot_modules: Vec<usize>,
    /// Untyped region backing the transitional managed runtime. Boot objects
    /// use the kernel frame pool until `init_root` selects this region.
    managed_untyped: Option<ObjectId>,
}
static STORE: SingleCore<Store> = SingleCore::new(Store {
    objects: ObjectTable::new(),
    parents: BTreeMap::new(),
    managed: BTreeSet::new(),
    next_serial: 1,
    caps: 0,
    boot_untyped: Vec::new(),
    boot_modules: Vec::new(),
    managed_untyped: None,
});

pub(crate) fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    f(&mut STORE.borrow_mut())
}

/// Test-only raw object publication; production objects come from retype,
/// boot partitioning or the IRQ/ROOT setup paths.
#[cfg(feature = "kernel-test")]
pub(crate) fn test_insert(object: Object) -> Option<ObjectId> {
    with_store(|store| store.objects.insert(object))
}

/// Test-only raw removal; the caller applies any object-specific finalizers.
#[cfg(feature = "kernel-test")]
pub(crate) fn test_remove(id: ObjectId) -> Option<Object> {
    with_store(|store| store.objects.remove(id))
}

impl Store {
    fn cnode(&self, id: ObjectId) -> Result<&CNode> {
        match self.objects.get(id) {
            Some(Object::CNode(cnode)) => Ok(cnode),
            _ => Err(INVALID_CAPABILITY),
        }
    }
    fn cnode_mut(&mut self, id: ObjectId) -> Result<&mut CNode> {
        match self.objects.get_mut(id) {
            Some(Object::CNode(cnode)) => Ok(cnode),
            _ => Err(INVALID_CAPABILITY),
        }
    }
    fn cap(&self, cspace: ObjectId, slot: u64) -> Result<Cap> {
        if slot == 0 || slot >= CNODE_SLOTS {
            return Err(NOT_FOUND);
        }
        self.cnode(cspace)?
            .slots
            .get(&(slot as u16))
            .cloned()
            .ok_or(NOT_FOUND)
    }
    fn insert_cap(
        &mut self,
        cspace: ObjectId,
        slot: u64,
        object: ObjectId,
        rights: u64,
        parent: u64,
        badge: u64,
    ) -> Result<()> {
        if slot == 0 || slot >= CNODE_SLOTS {
            return Err(RANGE_ERROR);
        }
        if self.caps >= MAX_CAPS || self.parents.len() >= MAX_DERIVATIONS {
            return Err(NO_MEMORY);
        }
        let serial = self.next_serial;
        self.next_serial = serial.checked_add(1).ok_or(NO_MEMORY)?;
        let cnode = self.cnode_mut(cspace)?;
        if cnode.slots.contains_key(&(slot as u16)) {
            return Err(ALREADY_MAPPED);
        }
        cnode.slots.insert(
            slot as u16,
            Cap {
                serial,
                object,
                rights,
                badge,
                mapping: None,
            },
        );
        self.parents.insert(serial, parent);
        self.caps += 1;
        Ok(())
    }
    fn remove_cap(&mut self, cspace: ObjectId, slot: u64) -> Option<Cap> {
        let cap = self.cnode_mut(cspace).ok()?.slots.remove(&(slot as u16))?;
        self.caps = self.caps.saturating_sub(1);
        Some(cap)
    }
    /// Move a capability without changing its derivation identity or mapping.
    fn move_cap(
        &mut self,
        source: ObjectId,
        source_slot: u64,
        destination: ObjectId,
        destination_slot: u64,
    ) -> Result<()> {
        if destination_slot == 0 || destination_slot >= CNODE_SLOTS {
            return Err(RANGE_ERROR);
        }
        if self
            .cnode(destination)?
            .slots
            .contains_key(&(destination_slot as u16))
        {
            return Err(ALREADY_MAPPED);
        }
        let cap = self
            .cnode_mut(source)?
            .slots
            .remove(&(source_slot as u16))
            .ok_or(NOT_FOUND)?;
        self.cnode_mut(destination)?
            .slots
            .insert(destination_slot as u16, cap);
        Ok(())
    }
    fn empty_slot(&self, cspace: ObjectId) -> Result<u64> {
        let cnode = self.cnode(cspace)?;
        (FIRST_FREE_SLOT..CNODE_SLOTS)
            .find(|slot| !cnode.slots.contains_key(&(*slot as u16)))
            .ok_or(NO_MEMORY)
    }
    fn kind(&self, id: ObjectId) -> Result<ObjectKind> {
        self.objects
            .get(id)
            .map(Object::kind)
            .ok_or(INVALID_CAPABILITY)
    }
    fn vspace(&self, id: ObjectId) -> Result<&AddressSpace> {
        match self.objects.get(id) {
            Some(Object::VSpace(vspace)) => vspace.space.as_ref().ok_or(INVALID_CAPABILITY),
            _ => Err(INVALID_CAPABILITY),
        }
    }
    fn vspace_mut(&mut self, id: ObjectId) -> Result<&mut AddressSpace> {
        match self.objects.get_mut(id) {
            Some(Object::VSpace(vspace)) => vspace.space.as_mut().ok_or(INVALID_CAPABILITY),
            _ => Err(INVALID_CAPABILITY),
        }
    }
    fn vspace_opt_mut(&mut self, id: ObjectId) -> Option<&mut AddressSpace> {
        match self.objects.get_mut(id) {
            Some(Object::VSpace(vspace)) => vspace.space.as_mut(),
            _ => None,
        }
    }
    fn frame_ref(&self, id: ObjectId) -> Result<FrameRef> {
        match self.objects.get(id) {
            Some(Object::Frame(frame)) | Some(Object::PageTable(frame)) => Ok(FrameRef::new(
                id,
                frame.address(),
                frame.physical(),
                frame.is_device(),
            )),
            _ => Err(INVALID_CAPABILITY),
        }
    }
    fn untyped(&self, id: ObjectId) -> Result<&Untyped> {
        match self.objects.get(id) {
            Some(Object::Untyped(untyped)) => Ok(untyped),
            _ => Err(INVALID_CAPABILITY),
        }
    }
    fn untyped_mut(&mut self, id: ObjectId) -> Result<&mut Untyped> {
        match self.objects.get_mut(id) {
            Some(Object::Untyped(untyped)) => Ok(untyped),
            _ => Err(INVALID_CAPABILITY),
        }
    }
    /// Carve `size` bytes from an Untyped region, returning `(physical, offset)`.
    fn untyped_allocate(
        &mut self,
        id: ObjectId,
        size: usize,
        align: usize,
    ) -> Result<(usize, usize)> {
        let base = self.untyped(id)?.physical();
        let physical = self
            .untyped_mut(id)?
            .allocate(size, align)
            .ok_or(NO_MEMORY)?;
        Ok((physical, physical - base))
    }
    /// Reserve and zero ordinary Untyped bytes for a metadata-backed object.
    fn untyped_reserve(
        &mut self,
        id: ObjectId,
        size: usize,
        align: usize,
    ) -> Result<(usize, usize)> {
        let (physical, offset) = self.untyped_allocate(id, size, align)?;
        if !self.untyped(id)?.is_device() {
            untyped::zero(physical, size);
        }
        Ok((physical, offset))
    }
    fn insert_page(&mut self, frame: Frame, page_table: bool) -> Result<FrameRef> {
        let virt = frame.address();
        let physical = frame.physical();
        let device = frame.is_device();
        let object = if page_table {
            Object::PageTable(frame)
        } else {
            Object::Frame(frame)
        };
        let id = self.objects.insert(object).ok_or(NO_MEMORY)?;
        Ok(FrameRef::new(id, virt, physical, device))
    }
    /// Allocate a page from an Untyped region and publish it as a frame object.
    fn new_untyped_frame(&mut self, untyped: ObjectId, page_table: bool) -> Result<FrameRef> {
        let (physical, offset) = self.untyped_allocate(untyped, PAGE_SIZE, PAGE_SIZE)?;
        let is_device = self.untyped(untyped)?.is_device();
        let frame = Frame::from_untyped(physical, is_device).map_err(|e| e as u64)?;
        let virt = frame.address();
        if !is_device {
            // Cross-task reuse must not leak the previous object's contents.
            // SAFETY: the page is exclusively owned by the object table and
            // reachable through the validated direct map.
            unsafe { core::ptr::write_bytes(virt as *mut u8, 0, PAGE_SIZE) };
        }
        let object = if page_table {
            Object::PageTable(frame)
        } else {
            Object::Frame(frame)
        };
        let id = self
            .objects
            .insert_owned(
                object,
                Some(ObjectOwner {
                    untyped,
                    offset,
                    size: PAGE_SIZE,
                }),
            )
            .ok_or(NO_MEMORY)?;
        Ok(FrameRef::new(id, virt, physical, is_device))
    }
    /// `untyped == None` uses the kernel boot pool; `Some` carves user memory.
    fn new_frame(&mut self, untyped: Option<ObjectId>, page_table: bool) -> Result<FrameRef> {
        match untyped {
            Some(untyped) => self.new_untyped_frame(untyped, page_table),
            None => {
                let frame = Frame::allocate().map_err(|e| e as u64)?;
                self.insert_page(frame, page_table)
            }
        }
    }
    fn new_loaded_frame(&mut self, physical: usize) -> Result<FrameRef> {
        let frame = Frame::take_boot(physical).map_err(|e| e as u64)?;
        self.insert_page(frame, false)
    }
    fn new_vspace(&mut self, untyped: Option<ObjectId>) -> Result<ObjectId> {
        let root = self.new_frame(untyped, false)?;
        let l1 = match self.new_frame(untyped, false) {
            Ok(frame) => frame,
            Err(error) => {
                self.objects.remove(root.id());
                return Err(error);
            }
        };
        let l2 = match self.new_frame(untyped, false) {
            Ok(frame) => frame,
            Err(error) => {
                self.objects.remove(root.id());
                self.objects.remove(l1.id());
                return Err(error);
            }
        };
        let space = AddressSpace::new(root, l1, l2).map_err(|e| e as u64)?;
        let record = untyped.map(|untyped| ObjectOwner {
            untyped,
            offset: 0,
            size: 0,
        });
        self.objects
            .insert_owned(
                Object::VSpace(VSpace {
                    assigned: false,
                    space: Some(space),
                }),
                record,
            )
            .ok_or(NO_MEMORY)
    }
    fn map_vspace(
        &mut self,
        id: ObjectId,
        va: usize,
        len: usize,
        permissions: u64,
        pinned: bool,
        loaded: Option<usize>,
    ) -> Result<()> {
        let untyped = self.managed_untyped;
        let watermark = match untyped {
            Some(untyped) => Some(self.untyped(untyped)?.free_offset()),
            None => None,
        };
        let plan = self
            .vspace(id)?
            .plan_map(va, len, permissions, pinned)
            .map_err(|e| e as u64)?;
        let mut tables = Vec::new();
        let mut pages = Vec::new();
        let result = (|| -> Result<()> {
            tables
                .try_reserve(plan.tables.len())
                .map_err(|_| NO_MEMORY)?;
            for _ in 0..plan.tables.len() {
                tables.push(self.new_frame(untyped, true)?);
            }
            pages.try_reserve(plan.pages).map_err(|_| NO_MEMORY)?;
            for offset in 0..plan.pages {
                pages.push(match loaded {
                    Some(base) => self.new_loaded_frame(base + offset * PAGE_SIZE)?,
                    None => self.new_frame(untyped, false)?,
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Remove the partial allocation and rewind the watermark so a
            // failed mapping leaves no bytes charged and no stale objects.
            for frame in tables.iter().chain(pages.iter()) {
                self.objects.remove(frame.id());
            }
            if let (Some(untyped), Some(watermark)) = (untyped, watermark)
                && let Some(Object::Untyped(region)) = self.objects.get_mut(untyped)
            {
                region.reset_to(watermark);
            }
            return Err(error);
        }
        self.vspace_mut(id)?
            .install(plan, tables, pages)
            .map_err(|e| e as u64)
    }
}

fn resolve(slot: u64) -> Result<(Cap, ObjectKind)> {
    let cspace = api::current_cspace();
    with_store(|store| {
        let cap = store.cap(cspace, slot)?;
        let kind = store.kind(cap.object)?;
        Ok((cap, kind))
    })
}
fn tcb(slot: u64) -> Result<u64> {
    let (cap, kind) = resolve(slot)?;
    if cap.rights & RIGHTS_WRITE == 0 {
        return Err(PERMISSION_DENIED);
    }
    if kind != ObjectKind::Tcb {
        return Err(INVALID_CAPABILITY);
    }
    with_store(|store| match store.objects.get(cap.object) {
        Some(Object::Tcb(task)) => Ok(*task),
        _ => Err(INVALID_CAPABILITY),
    })
}
fn vspace(slot: u64) -> Result<ObjectId> {
    let (cap, kind) = resolve(slot)?;
    if cap.rights & RIGHTS_WRITE == 0 {
        return Err(PERMISSION_DENIED);
    }
    if kind != ObjectKind::VSpace {
        return Err(INVALID_CAPABILITY);
    }
    with_store(|store| match store.objects.get(cap.object) {
        Some(Object::VSpace(vspace)) if vspace.space.is_some() && vspace.assigned => Ok(cap.object),
        _ => Err(INVALID_CAPABILITY),
    })
}
fn cnode(slot: u64) -> Result<ObjectId> {
    let (cap, kind) = resolve(slot)?;
    if cap.rights & RIGHTS_WRITE == 0 {
        return Err(PERMISSION_DENIED);
    }
    if kind != ObjectKind::CNode {
        return Err(INVALID_CAPABILITY);
    }
    Ok(cap.object)
}

/// Read-only address-space access for the task layer and message marshalling.
pub(crate) fn with_vspace<T>(
    id: ObjectId,
    operation: impl FnOnce(&AddressSpace) -> core::result::Result<T, MemoryError>,
) -> Result<T> {
    with_store(|store| operation(store.vspace(id)?).map_err(|e| e as u64))
}

/// Resolve a capability for an IPC operation. Unlike the object-invocation
/// path there is no blanket `Write` check: send, receive and grant inspect
/// rights per operation, so the caller receives the full record.
pub(crate) fn lookup(slot: u64) -> Result<(ObjectKind, ObjectId, u64, u64)> {
    let (cap, kind) = resolve(slot)?;
    Ok((kind, cap.object, cap.badge, cap.rights))
}

/// Resolve a capability in an explicit CSpace (receiver-side receive specs and
/// cross-task transfer).
pub(crate) fn lookup_in(cspace: ObjectId, slot: u64) -> Result<(ObjectKind, ObjectId, u64, u64)> {
    with_store(|store| {
        let cap = store.cap(cspace, slot)?;
        Ok((store.kind(cap.object)?, cap.object, cap.badge, cap.rights))
    })
}

/// Is `slot` empty in `cspace`? Message cap transfer validates every landing
/// slot before the first one is written.
pub(crate) fn slot_empty(cspace: ObjectId, slot: u64) -> Result<bool> {
    with_store(|store| Ok(store.cap(cspace, slot).is_err()))
}

/// Access an endpoint/notification wait queue by object identity.
pub(crate) fn with_wait_queue<T>(
    id: ObjectId,
    operation: impl FnOnce(&mut WaitQueue) -> T,
) -> Result<T> {
    with_store(|store| match store.objects.get_mut(id) {
        Some(Object::Endpoint(endpoint)) => Ok(operation(&mut endpoint.queue)),
        Some(Object::Notification(notification)) => Ok(operation(&mut notification.queue)),
        _ => Err(INVALID_CAPABILITY),
    })
}
pub(crate) fn with_notification_bits<T>(
    id: ObjectId,
    operation: impl FnOnce(&mut u64) -> T,
) -> Result<T> {
    with_store(|store| match store.objects.get_mut(id) {
        Some(Object::Notification(notification)) => Ok(operation(&mut notification.bits)),
        _ => Err(INVALID_CAPABILITY),
    })
}
/// Transfer the capabilities of one message. Every check — Grant on each
/// source, empty landing slots, derivation budget — happens before the first
/// insert, so a failed transfer leaves no partial delivery.
pub(crate) fn transfer_caps(
    source_cspace: ObjectId,
    sources: &[u64],
    destination: ObjectId,
    first_slot: u64,
) -> Result<()> {
    with_store(|store| {
        let mut staged = Vec::new();
        if store.caps + sources.len() > MAX_CAPS
            || store.parents.len() + sources.len() > MAX_DERIVATIONS
        {
            return Err(NO_MEMORY);
        }
        for (offset, &slot) in sources.iter().enumerate() {
            let cap = store.cap(source_cspace, slot)?;
            if cap.rights & RIGHTS_GRANT == 0 {
                return Err(PERMISSION_DENIED);
            }
            if store
                .cnode(destination)?
                .slots
                .contains_key(&((first_slot + offset as u64) as u16))
            {
                return Err(ALREADY_MAPPED);
            }
            staged.push(cap);
        }
        for (offset, cap) in staged.iter().enumerate() {
            store.insert_cap(
                destination,
                first_slot + offset as u64,
                cap.object,
                cap.rights,
                cap.serial,
                cap.badge,
            )?;
        }
        Ok(())
    })
}
/// Mutable address-space access after the caller has authorized the task.
pub(crate) fn edit_vspace<T>(
    id: ObjectId,
    operation: impl FnOnce(&mut AddressSpace) -> core::result::Result<T, MemoryError>,
) -> Result<T> {
    with_store(|store| operation(store.vspace_mut(id)?).map_err(|e| e as u64))
}
pub(crate) fn map_vspace(
    id: ObjectId,
    va: usize,
    len: usize,
    permissions: u64,
    pinned: bool,
) -> Result<()> {
    let result = with_store(|store| store.map_vspace(id, va, len, permissions, pinned, None));
    if result.is_err() {
        // A failed mapping may have allocated frames before the failure; they
        // are unreferenced and must be reclaimed by the next sweep.
        request_collect();
    }
    result
}
pub(crate) fn vspace_root(id: ObjectId) -> Result<usize> {
    with_store(|store| store.vspace(id).map(AddressSpace::root))
}
pub(crate) fn vspace_frame_at(id: ObjectId, va: usize) -> Result<FrameRef> {
    with_store(|store| store.vspace(id)?.frame_at(va).map_err(|e| e as u64))
}

/// Create a standalone VSpace object. The caller must bind it to a capability
/// or thread before collection runs.
pub(crate) fn create_vspace() -> Result<ObjectId> {
    with_store(|store| {
        let untyped = store.managed_untyped;
        store.new_vspace(untyped)
    })
}

pub(crate) fn init_root(task: u64, vspace: ObjectId, ipc: usize, untyped_start: u64) -> ObjectId {
    with_store(|store| {
        if let Some(Object::VSpace(v)) = store.objects.get_mut(vspace) {
            v.assigned = true;
        }
        let tcb = store.objects.insert(Object::Tcb(task)).expect("root TCB");
        let cnode = store
            .objects
            .insert(Object::CNode(CNode::new()))
            .expect("root CNode");
        let runtime = store.objects.insert(Object::Runtime).expect("root Runtime");
        let asid = store
            .objects
            .insert(Object::AsidPool)
            .expect("root ASID pool");
        let irq_control = store
            .objects
            .insert(Object::IrqControl)
            .expect("root IRQControl");
        let page = store
            .vspace(vspace)
            .expect("root VSpace")
            .frame_at(ipc)
            .expect("initial IPC frame")
            .id();
        for (slot, object) in [
            (INIT_TCB, tcb),
            (INIT_CNODE, cnode),
            (INIT_VSPACE, vspace),
            (INIT_IRQ_CONTROL, irq_control),
            (INIT_RUNTIME, runtime),
            (INIT_ASID_POOL, asid),
            (INIT_IPC_BUFFER, page),
        ] {
            store
                .insert_cap(cnode, slot, object, RIGHTS_ALL, 0, 0)
                .expect("root capability");
        }
        // Publish every boot-partitioned physical region as a contiguous range
        // of Untyped capabilities. The largest ordinary region also backs the
        // transitional managed runtime, so its allocations are accounted.
        let mut managed: Option<(ObjectId, u8)> = None;
        for (index, &id) in store.boot_untyped.clone().iter().enumerate() {
            store
                .insert_cap(cnode, untyped_start + index as u64, id, RIGHTS_ALL, 0, 0)
                .expect("root Untyped capability");
            if let Object::Untyped(untyped) = store.objects.get(id).expect("boot Untyped") {
                if !untyped.is_device()
                    && managed.is_none_or(|(_, bits)| untyped.size_bits() > bits)
                {
                    managed = Some((id, untyped.size_bits()));
                }
            }
        }
        // Publish the boot-module archive pages as read-only Frame caps so
        // the root task can map and parse its own boot modules.
        for (index, &physical) in store.boot_modules.clone().iter().enumerate() {
            let frame = store.new_loaded_frame(physical).expect("module frame");
            store
                .insert_cap(
                    cnode,
                    kernel_abi::INIT_BOOT_MODULES + index as u64,
                    frame.id(),
                    RIGHTS_READ,
                    0,
                    0,
                )
                .expect("root module capability");
        }
        // The largest ordinary region also backs the transitional managed
        // runtime, so its allocations are billed like any other Untyped use.
        store.managed_untyped = managed.map(|(id, _)| id);
        store.managed.insert(task);
        cnode
    })
}

/// Managed tasks receive an isolated CSpace. Authority is represented by caps,
/// never inferred from a user's integer matching a global task ID.
pub(crate) fn publish_task(task: u64, vspace: ObjectId, ipc: usize) -> Result<u64> {
    let parent = api::current_cspace();
    let root = vspace_root(vspace)?;
    let (slot, child_cspace) = with_store(|store| {
        if store.objects.len() + 4 > MAX_OBJECTS
            || store.caps + 8 > MAX_CAPS
            || store.parents.len() + 8 > MAX_DERIVATIONS
        {
            return Err(NO_MEMORY);
        }
        let slot = store.empty_slot(parent)?;
        let runtime = store.cap(parent, INIT_RUNTIME)?;
        let tcb = store.objects.insert(Object::Tcb(task)).ok_or(NO_MEMORY)?;
        let cnode = store
            .objects
            .insert(Object::CNode(CNode::new()))
            .ok_or(NO_MEMORY)?;
        store.insert_cap(parent, slot, tcb, RIGHTS_ALL, 0, 0)?;
        let source = store.cap(parent, slot)?;
        store.insert_cap(cnode, INIT_TCB, tcb, RIGHTS_ALL, source.serial, 0)?;
        store.insert_cap(cnode, INIT_CNODE, cnode, RIGHTS_ALL, 0, 0)?;
        store.insert_cap(cnode, INIT_VSPACE, vspace, RIGHTS_ALL, 0, 0)?;
        // Managed children receive no Untyped authority; the runtime, not the
        // child, decides how the parent's physical memory is spent.
        store.insert_cap(
            cnode,
            INIT_RUNTIME,
            runtime.object,
            runtime.rights,
            runtime.serial,
            0,
        )?;
        let page = store
            .vspace(vspace)?
            .frame_at(ipc)
            .map_err(|e| e as u64)?
            .id();
        store.insert_cap(cnode, INIT_IPC_BUFFER, page, RIGHTS_ALL, 0, 0)?;
        store.managed.insert(task);
        Ok::<_, u64>((slot, cnode))
    })?;
    api::configure(task, child_cspace, vspace, root, ipc, 0)?;
    Ok(slot)
}

/// Thread retirement. Only the thread's own execution state dies with it (the
/// scheduler drops the `Execution`/kernel stack before calling this); the
/// CSpace and VSpace it referenced stay reachable for sibling threads and
/// capabilities, and collection reclaims them after the last reference
/// disappears (docs/fault-handler.md §3.3, §8). Managed tasks additionally
/// release the runtime-installed IPC buffer capability from their CSpace.
pub(crate) fn retire_thread(task: u64, cspace: Option<ObjectId>) {
    with_store(|store| {
        if !store.managed.contains(&task) {
            return;
        }
        log::info!("DBG retire_thread strips task={:#x}", task);
        if let Some(cspace) = cspace {
            store.remove_cap(cspace, INIT_IPC_BUFFER);
        }
    });
    request_collect();
}

/// Release a destroyed thread's CSpace binding. `shared` must be computed by
/// the caller (which holds the scheduler borrow): `true` when another live
/// thread references the same CSpace. A thread group shares one CSpace, and
/// destroying one member must not leave its siblings with a dangling identity
/// (docs/thread-group.md §2.4).
pub(crate) fn forget_task(task: u64, cspace: Option<ObjectId>, shared: bool) {
    with_store(|store| {
        if store.managed.remove(&task)
            && let Some(cspace) = cspace
            && !shared
        {
            store.objects.remove(cspace);
        }
    });
    request_collect();
}

/// Destroy the object payloads a terminated thread owned. Used by the explicit
/// runtime policy, which owns standard TCBs as well as managed ones. A VSpace
/// still referenced by a surviving thread — a thread-group sibling — is left
/// in place; the thread itself is fully retired.
pub(crate) fn release_task_objects(task: u64, vspace: Option<ObjectId>) {
    // After `api::destroy` the target no longer references anything, so any
    // remaining thread root naming `vspace` belongs to a surviving sibling.
    let shared = vspace.is_some_and(|id| api::thread_roots().contains(&id));
    with_store(|store| {
        let ids: Vec<ObjectId> = store
            .objects
            .iter()
            .filter_map(|(id, object)| match object {
                Object::Tcb(owner) if *owner == task => Some(id),
                Object::VSpace(_) if !shared && vspace.is_some_and(|v| v == id) => Some(id),
                _ => None,
            })
            .collect();
        for id in &ids {
            store.objects.remove(*id);
        }
        if ids.is_empty() {
            return;
        }
        // Capabilities naming a destroyed object must become absent, not stale:
        // resolve reports NOT_FOUND instead of an invalid capability.
        let cspaces: Vec<ObjectId> = store
            .objects
            .iter()
            .filter_map(|(id, object)| matches!(object, Object::CNode(_)).then_some(id))
            .collect();
        for cspace in cspaces {
            if let Ok(cnode) = store.cnode_mut(cspace) {
                cnode.slots.retain(|_, cap| !ids.contains(&cap.object));
            }
        }
    });
    request_collect();
}

/// Collection is demand-driven: releasing references marks the store dirty and
/// the next safe boundary sweeps unreachable objects. This keeps the bounded
/// but non-trivial mark-and-sweep cost off the syscall fast path.
static mut COLLECT_PENDING: bool = false;
pub(crate) fn request_collect() {
    // SAFETY: single CPU, IRQ masked.
    unsafe { COLLECT_PENDING = true };
}
fn take_collect_request() -> bool {
    // SAFETY: single CPU, IRQ masked.
    unsafe {
        let pending = COLLECT_PENDING;
        COLLECT_PENDING = false;
        pending
    }
}
/// Sweep unreachable objects if a release has been recorded. Safe to call at
/// any IRQ-masked boundary with no store or scheduler borrow held.
pub(crate) fn collect_if_requested() {
    if take_collect_request() {
        collect();
    }
}

/// Mark-and-sweep collection. Roots are capabilities plus the CSpace/VSpace
/// referenced by scheduler threads (a thread group keeps its shared objects
/// alive while any member, or any capability, survives); an address space
/// keeps its mapped frames reachable. Dead objects are removed in waves, which
/// frees frames only after their last reference disappears.
pub(crate) fn collect() {
    loop {
        let thread_roots = api::thread_roots();
        let (tasks, changed) = with_store(|store| {
            let mut live = BTreeSet::new();
            for (_, object) in store.objects.iter() {
                if let Object::CNode(cnode) = object {
                    for cap in cnode.slots.values() {
                        live.insert(cap.object);
                    }
                }
                // A bound IRQHandler keeps its Notification object alive even
                // when no capability names it anymore (docs/irq.md §6); the
                // binding dies with the handler or an explicit Clear.
                if let Object::IrqHandler(handler) = object
                    && let Some((notification, _)) = handler.bound()
                {
                    live.insert(notification);
                }
            }
            for id in thread_roots.iter().copied() {
                live.insert(id);
            }
            let mut queue: Vec<ObjectId> = live.iter().copied().collect();
            while let Some(id) = queue.pop() {
                if let Some(Object::VSpace(vspace)) = store.objects.get(id)
                    && let Some(space) = vspace.space.as_ref()
                {
                    for frame in space.frame_refs() {
                        if live.insert(frame.id()) {
                            queue.push(frame.id());
                        }
                    }
                }
            }
            let dead: Vec<ObjectId> = store
                .objects
                .ids()
                .into_iter()
                .filter(|id| !live.contains(id))
                .collect();
            let changed = !dead.is_empty();
            let mut tasks = Vec::new();
            for id in dead {
                // A collected endpoint would strand its waiters in a queue that
                // no longer exists; suspend them at the cancellation point.
                if matches!(
                    store.objects.get(id),
                    Some(Object::Endpoint(_)) | Some(Object::Notification(_))
                ) {
                    api::suspend_blocked_on(id);
                }
                if let Some(object) = store.objects.remove(id) {
                    // A collected handler must release its line: drop the
                    // delivery index and disable the source (docs/irq.md §5).
                    if let Object::IrqHandler(handler) = &object {
                        irq::retire(handler);
                    }
                    if let Object::Tcb(task) = object {
                        tasks.push(task);
                    }
                }
            }
            let mut needed = BTreeSet::new();
            for (_, object) in store.objects.iter() {
                if let Object::CNode(cnode) = object {
                    for cap in cnode.slots.values() {
                        let mut serial = cap.serial;
                        while serial != 0 && needed.insert(serial) {
                            serial = store.parents.get(&serial).copied().unwrap_or(0);
                        }
                    }
                }
            }
            store.parents.retain(|serial, _| needed.contains(serial));
            store.caps = store
                .objects
                .iter()
                .filter_map(|(_, object)| match object {
                    Object::CNode(cnode) => Some(cnode.slots.len()),
                    _ => None,
                })
                .sum();
            // The managed runtime has no per-object free list: once it owns no
            // objects, its whole region can be rewound and reused.
            if let Some(managed) = store.managed_untyped
                && store.objects.children(managed).is_empty()
                && let Some(Object::Untyped(region)) = store.objects.get_mut(managed)
                && region.free_offset() > 0
            {
                log::info!("DBG collect: managed region reset");
                region.reset();
                region.clear();
            }
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
    // SAFETY: single CPU, IRQ masked. Clear requests raised while finalising
    // (task destruction re-requests collection) so the next syscall boundary
    // does not run a redundant sweep that would perturb observable state.
    unsafe { COLLECT_PENDING = false };
}

// Boot-time object construction. The loader owns these mappings before any user
// task exists; afterwards the objects live under the same collection rules.

/// Create the boot-partitioned Untyped objects. Their capabilities are installed
/// by `init_root`; the descriptor order matches the BootInfo Untyped list.
pub(crate) fn boot_untyped(regions: &[(usize, u8, bool)]) -> Result<()> {
    with_store(|store| {
        for &(physical, size_bits, is_device) in regions {
            let id = store
                .objects
                .insert(Object::Untyped(Untyped::new(
                    physical, size_bits, is_device,
                )))
                .ok_or(NO_MEMORY)?;
            store.boot_untyped.push(id);
        }
        Ok(())
    })
}

/// Record the boot-module archive pages. `init_root` later installs one
/// read-only Frame capability per page starting at `INIT_BOOT_MODULES`.
pub(crate) fn boot_modules(regions: core::ops::Range<usize>) {
    with_store(|store| {
        store.boot_modules = regions.step_by(crate::memory::PAGE_SIZE).collect();
    });
}

/// Free ordinary Untyped bytes across every live region, in bytes.
pub(crate) fn available_untyped() -> usize {
    with_store(|store| {
        store
            .objects
            .iter()
            .filter_map(|(_, object)| match object {
                Object::Untyped(untyped) if !untyped.is_device() => Some(untyped.remaining()),
                _ => None,
            })
            .sum()
    })
}

pub(crate) fn boot_vspace() -> Result<ObjectId> {
    with_store(|store| store.new_vspace(None))
}
pub(crate) fn boot_map_loaded(
    vspace: ObjectId,
    va: usize,
    physical: usize,
    len: usize,
    permissions: u64,
) -> Result<()> {
    with_store(|store| store.map_vspace(vspace, va, len, permissions, false, Some(physical)))
}
pub(crate) fn boot_map(
    vspace: ObjectId,
    va: usize,
    len: usize,
    permissions: u64,
    pinned: bool,
) -> Result<()> {
    with_store(|store| store.map_vspace(vspace, va, len, permissions, pinned, None))
}
pub(crate) fn boot_write(vspace: ObjectId, va: usize, bytes: &[u8]) -> Result<()> {
    with_store(|store| {
        store
            .vspace_mut(vspace)?
            .initialize(va, bytes)
            .map_err(|e| e as u64)
    })
}
