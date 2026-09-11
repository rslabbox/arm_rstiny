//! Kernel IRQ routing, separate from controller operations and task switching.
//!
//! seL4 GICv3 protocol (docs/irq.md §5): delivery claims the line
//! (pending → active), signals the bound Notification and drops priority only,
//! so the line stays active — the implicit mask — until the driver's Ack
//! deactivates it. Kernel-owned lines (timer) and unknown sources recycle
//! their claim immediately; unknown sources are additionally disabled so an
//! asserted level source cannot storm the kernel (seL4 IRQInactive masking).
use crate::{arch::machine::gic, object::irq, task::tick};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Continue,
    Reschedule,
}

/// Service one IRQ with local exceptions masked; no borrow spans the handler.
/// All hardware claims are discharged before returning a scheduling decision.
pub(crate) fn handle() -> Outcome {
    let Some(active) = gic::claim() else {
        return Outcome::Continue;
    };
    let id = active.id();
    if id == tick::IRQ {
        // Kernel-owned scheduler tick: rearm clears the source, then the
        // claim is recycled immediately.
        tick::on_interrupt();
        gic::priority_and_deactivate(active);
        return Outcome::Reschedule;
    }
    if let Some((notification, badge)) = irq::delivery_target(id.to_u32()) {
        // Notify the driver and keep the line active: GIC active state blocks
        // re-delivery until the driver acknowledges, so an uncleared source
        // cannot re-enter the kernel within one driver pass.
        let woke = crate::api::signal_notification(notification, badge);
        gic::priority_drop(active);
        return if woke {
            Outcome::Reschedule
        } else {
            Outcome::Continue
        };
    }
    // Unknown source: recycle the claim and disable the line.
    gic::priority_and_deactivate(active);
    gic::set_enable(id, false);
    Outcome::Continue
}
