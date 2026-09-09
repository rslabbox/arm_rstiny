//! Kernel IRQ routing, separate from controller operations and task switching.
use crate::{arch::machine::gic, task::tick};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Continue,
    Reschedule,
}

/// Service one IRQ with local exceptions masked; no borrow spans the handler.
/// All hardware claims are completed before returning a scheduling decision.
pub(crate) fn handle() -> Outcome {
    let Some(active) = gic::claim() else {
        return Outcome::Continue;
    };
    let irq = active.id();
    let outcome = if irq == tick::IRQ {
        tick::on_interrupt();
        Outcome::Reschedule
    } else {
        // No registered consumer or user IRQ delivery exists yet. Disable the
        // source to avoid a storm, but still complete its active hardware claim.
        gic::set_enable(irq, false);
        Outcome::Continue
    };
    gic::complete(active);
    outcome
}
