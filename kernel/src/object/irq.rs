//! seL4-style device IRQ authorization and user-space delivery (docs/irq.md).
//!
//! [`IrqControl`] is a root-only singleton that derives [`IrqHandler`] objects
//! for the platform table's user-visible lines. A handler binds one
//! Notification; delivery signals it with the binding's badge and leaves the
//! line active in the GIC until the driver acknowledges. GICv3 split EOI makes
//! that active state the implicit mask: an unacknowledged level source cannot
//! storm the kernel, and only an explicit `Clear` (or collection) disables a
//! line. Users never touch the GIC: device Untyped never covers it.
//!
//! Only the seL4 wire labels ([`Invocation::IrqIssueIrqHandler`] and friends)
//! resolve capabilities; the `*_line`/`bind`/`acknowledge`/`clear` internals
//! are capability-free so the kernel tests can drive them directly.
use super::{
    Cap, MAX_CAPS, MAX_DERIVATIONS, MAX_OBJECTS, Object, ObjectId, ObjectKind, Result, cnode,
    resolve, with_store,
};
use crate::api::{Completion, Request};
use crate::arch::machine::gic::{self, Trigger};
use crate::utils::single_core::SingleCore;
use alloc::collections::BTreeMap;
use kernel_abi::*;

/// Priority of user-visible lines: below the scheduler tick (0x80), above the
/// idle default, so kernel work is never starved by device floods.
const USER_IRQ_PRIORITY: u8 = 0xa0;

/// One authorized interrupt line. Payload lives in the object table; the
/// capability layer only carries its [`ObjectId`]. The trigger type is not
/// stored: it is fixed by the platform table and programmed into the GIC at
/// `Get` time, and nothing else may change it afterwards.
pub(crate) struct IrqHandler {
    /// INTID (SPI/PPI/SGI) of the line, as the user named it to `Get`.
    irq: u32,
    /// Bound Notification object plus the badge of the Notification capability
    /// used in `SetNotification` — that badge is what delivery carries, so a
    /// driver can share one Notification across several lines (docs/irq.md §2).
    bound: Option<(ObjectId, u64)>,
}
impl IrqHandler {
    /// The live binding, if any. Used by collection: a bound Notification is
    /// kept reachable until the binding dies.
    pub(super) fn bound(&self) -> Option<(ObjectId, u64)> {
        self.bound
    }
}

/// INTID → handler index. Serves the IRQ delivery fast path and the
/// duplicate-`Get` check without scanning the object table.
static HANDLERS: SingleCore<BTreeMap<u32, ObjectId>> = SingleCore::new(BTreeMap::new());

/// The platform table (docs/irq.md §3.1): a generated per-line list —
/// `config::IRQ_LINES` — decides what is authorizable and with which trigger.
/// The timer PPI and unmapped lines stay kernel-owned and are rejected here.
fn platform_trigger(intid: u32) -> Option<Trigger> {
    crate::config::user_irq_level(intid).map(
        |level| {
            if level { Trigger::Level } else { Trigger::Edge }
        },
    )
}

/// `IRQControl_Get` (label 26): authorize `words = [irq, index, depth]` into
/// `caps = [target CNode]`. A line may only be taken once while its handler
/// exists (`REVOKE_FIRST`, seL4's name); the destination slot must be empty.
pub(super) fn issue(cap: &Cap, request: &Request) -> Result<Completion> {
    request.require(3, 1)?;
    let [irq, index, depth] = [request.words[0], request.words[1], request.words[2]];
    if depth != 64 {
        return Err(INVALID_ARGUMENT);
    }
    let Ok(intid) = u32::try_from(irq) else {
        return Err(INVALID_ARGUMENT);
    };
    let node = cnode(request.caps[0])?;
    issue_line(cap.serial, node, index, intid)?;
    Ok(Completion::done(None))
}

/// Authorize `intid` into `(node, slot)`, deriving from `parent`. The line is
/// left enabled: a successful `Get` is seL4's `setIRQState(IRQSignal)`.
pub(crate) fn issue_line(parent: u64, node: ObjectId, slot: u64, intid: u32) -> Result<ObjectId> {
    let Some(trigger) = platform_trigger(intid) else {
        return Err(INVALID_ARGUMENT);
    };
    if slot == 0 || slot >= CNODE_SLOTS {
        return Err(RANGE_ERROR);
    }
    with_store(|store| {
        if store.objects.len() + 1 > MAX_OBJECTS
            || store.caps + 1 > MAX_CAPS
            || store.parents.len() + 1 > MAX_DERIVATIONS
        {
            return Err(NO_MEMORY);
        }
        if store.cnode(node)?.slots.contains_key(&(slot as u16)) {
            return Err(ALREADY_MAPPED);
        }
        if HANDLERS.borrow_mut().contains_key(&intid) {
            return Err(REVOKE_FIRST);
        }
        Ok(())
    })?;
    let id = with_store(|store| {
        let id = store
            .objects
            .insert(Object::IrqHandler(IrqHandler {
                irq: intid,
                bound: None,
            }))
            .ok_or(NO_MEMORY)?;
        match store.insert_cap(node, slot, id, RIGHTS_ALL, parent, 0) {
            Ok(()) => Ok(id),
            // Undo the object so a failed Get leaves nothing behind.
            Err(error) => {
                store.objects.remove(id);
                HANDLERS.borrow_mut().remove(&intid);
                Err(error)
            }
        }
    })?;
    HANDLERS.borrow_mut().insert(intid, id);
    // Program the platform trigger, then open the line for delivery.
    let irq = gic::from_intid(intid).ok_or(INVALID_ARGUMENT)?;
    gic::set_trigger(irq, trigger);
    gic::set_priority(irq, USER_IRQ_PRIORITY);
    gic::set_enable(irq, true);
    Ok(id)
}

/// `IRQHandler` dispatch (docs/irq.md §4): `SetNotification` (28),
/// `Ack` (27), `Clear` (29).
pub(super) fn handler_invoke(cap: &Cap, request: &Request) -> Result<Completion> {
    match request.label {
        n if n == Invocation::IrqSetIrqHandler as u64 => {
            request.require(0, 1)?;
            let (notification, kind) = resolve(request.caps[0])?;
            if kind != ObjectKind::Notification {
                return Err(INVALID_CAPABILITY);
            }
            bind(cap.object, notification.object, notification.badge)?;
        }
        n if n == Invocation::IrqAckIrq as u64 => {
            request.require(0, 0)?;
            acknowledge(cap.object)?;
        }
        n if n == Invocation::IrqClearIrqHandler as u64 => {
            request.require(0, 0)?;
            clear(cap.object)?;
        }
        _ => return Err(UNSUPPORTED),
    }
    Ok(Completion::done(None))
}

/// Bind (or replace) a handler's Notification. Binding re-enables the line:
/// a restarted driver's bind must recover a line its predecessor left
/// disabled by `Clear` (docs/irq.md §12.4: rebinding overwrites).
pub(crate) fn bind(handler: ObjectId, notification: ObjectId, badge: u64) -> Result<()> {
    let intid = with_store(|store| match store.objects.get_mut(handler) {
        Some(Object::IrqHandler(handler)) => {
            handler.bound = Some((notification, badge));
            Ok(handler.irq)
        }
        _ => Err(INVALID_CAPABILITY),
    })?;
    if let Some(irq) = gic::from_intid(intid) {
        gic::set_enable(irq, true);
    }
    Ok(())
}

/// `IRQHandler_Ack`: GICv3 deactivate (`ICC_DIR_EL1`). The line becomes
/// deliverable again; a level source the driver never cleared re-pends
/// immediately, which is the driver's bug to fix, not the kernel's.
pub(crate) fn acknowledge(handler: ObjectId) -> Result<()> {
    let intid = handler_irq(handler)?;
    gic::deactivate(gic::from_intid(intid).ok_or(INVALID_CAPABILITY)?);
    Ok(())
}

/// `IRQHandler_Clear` (docs/irq.md §12.3): drop the binding and quiesce the
/// line — disable it, then deactivate whatever a crashed driver left active,
/// so a re-authorized line is deliverable immediately. Idempotent.
pub(crate) fn clear(handler: ObjectId) -> Result<()> {
    let intid = with_store(|store| match store.objects.get_mut(handler) {
        Some(Object::IrqHandler(handler)) => {
            handler.bound = None;
            Ok(handler.irq)
        }
        _ => Err(INVALID_CAPABILITY),
    })?;
    if let Some(irq) = gic::from_intid(intid) {
        gic::set_enable(irq, false);
        gic::deactivate(irq);
    }
    Ok(())
}

/// Delivery fast path: the bound Notification and badge of `intid`, if its
/// handler holds one. Runs in the IRQ entry; the fallback covers an index
/// that outlived its (collected) handler between waves.
pub(crate) fn delivery_target(intid: u32) -> Option<(ObjectId, u64)> {
    let handler = HANDLERS.borrow_mut().get(&intid).copied()?;
    with_store(|store| match store.objects.get(handler) {
        Some(Object::IrqHandler(handler)) => handler.bound,
        _ => None,
    })
}

/// Collection finalizer: a handler whose last capability died loses its line —
/// drop the delivery index and quiesce the source like [`clear`].
pub(crate) fn retire(handler: &IrqHandler) {
    HANDLERS.borrow_mut().remove(&handler.irq);
    if let Some(irq) = gic::from_intid(handler.irq) {
        gic::set_enable(irq, false);
        gic::deactivate(irq);
    }
}

fn handler_irq(handler: ObjectId) -> Result<u32> {
    with_store(|store| match store.objects.get(handler) {
        Some(Object::IrqHandler(handler)) => Ok(handler.irq),
        _ => Err(INVALID_CAPABILITY),
    })
}
