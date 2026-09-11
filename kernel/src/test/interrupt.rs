//! Real GIC/timer lifecycle checks, before scheduler tick initialization.
use crate::{
    arch::machine::{
        gic::{self, IrqId, Trigger},
        time, timer,
    },
    config,
    interrupt::{self, Outcome},
    object::{
        self, CNode, Notification, Object,
        irq::{self as authorization},
    },
};
use aarch64_cpu::{asm::barrier, registers::*};
use kernel_abi::*;

#[unsafe(no_mangle)]
static mut IRQ_SELF_TEST_PASSED: u64 = 0;
fn synchronize() {
    barrier::dsb(barrier::SY);
    barrier::isb(barrier::SY);
}
fn complete_expected(irq: IrqId) {
    synchronize();
    let active = gic::claim().expect("enabled software-pending IRQ");
    assert_eq!(active.id(), irq);
    // Controller borrow was released by claim; ordinary operations remain usable.
    gic::set_pending(irq, false);
    gic::priority_and_deactivate(active);
    assert!(gic::claim().is_none(), "EOI failed to deactivate IRQ");
}
pub fn run() {
    timer::init();
    assert_eq!(interrupt::handle(), Outcome::Continue); // spurious, no EOI
    for irq in [IrqId::sgi(1), IrqId::spi(1)] {
        gic::set_enable(irq, false);
        gic::set_priority(irq, 0x80);
        if !irq.is_sgi() {
            gic::set_trigger(irq, Trigger::Edge);
        }
        gic::set_pending(irq, true);
        synchronize();
        assert!(gic::claim().is_none(), "disabled source was delivered");
        gic::set_enable(irq, true);
        complete_expected(irq);

        // Unknown sources are masked by dispatch and their claims completed.
        gic::set_pending(irq, true);
        synchronize();
        assert_eq!(interrupt::handle(), Outcome::Continue);
        gic::set_pending(irq, true);
        synchronize();
        assert!(gic::claim().is_none(), "unhandled IRQ was not masked");
        gic::set_enable(irq, true);
        complete_expected(irq); // also proves dispatch did not leave it active
        gic::set_enable(irq, false);
    }
    authorization_self_test();
    // The hardware driver supports long one-shot intervals, without TVAL's
    // signed 32-bit truncation, and can disarm an already expired source.
    let deadline = time::now() + (1u64 << 33);
    timer::arm(deadline);
    assert_eq!(CNTP_CVAL_EL0.get(), deadline);
    assert!(CNTP_CTL_EL0.is_set(CNTP_CTL_EL0::ENABLE));
    assert!(!CNTP_CTL_EL0.is_set(CNTP_CTL_EL0::ISTATUS));
    timer::arm(0);
    assert!(CNTP_CTL_EL0.is_set(CNTP_CTL_EL0::ISTATUS));
    timer::stop();
    assert!(!CNTP_CTL_EL0.is_set(CNTP_CTL_EL0::ENABLE));
    // SAFETY: single CPU, one-shot test completion published before user entry.
    unsafe {
        core::ptr::addr_of_mut!(IRQ_SELF_TEST_PASSED).write_volatile(1);
    }
}

/// Notification bits of a test notification object.
fn notification_bits(ntfn: object::ObjectId) -> u64 {
    object::with_notification_bits(ntfn, |bits| *bits).expect("test notification")
}

/// IRQControl/IRQHandler authorization and GICv3 delivery semantics
/// (docs/irq.md §10, I0/I1), driven through the same internals the wire
/// labels call: platform-table validation, duplicate Get, badge delivery,
/// active-state storm protection, Ack redelivery and Clear quiescing.
fn authorization_self_test() {
    // The generated per-line table decides everything: the first entries are
    // the VirtIO slot lines, and any INTID outside the table is rejected.
    let first = u32::try_from(config::IRQ_LINES[0].0).expect("platform INTID");
    let second = u32::try_from(config::IRQ_LINES[1].0).expect("platform INTID");
    let last =
        u32::try_from(config::IRQ_LINES[config::IRQ_LINES.len() - 1].0).expect("platform INTID");
    let slot_line = gic::from_intid(first).expect("platform line");
    let node = object::test_insert(Object::CNode(CNode::empty())).expect("test CNode");
    let control = object::test_insert(Object::IrqControl).expect("test IRQControl");
    let ntfn =
        object::test_insert(Object::Notification(Notification::new())).expect("test notification");
    // Lines outside the platform table are rejected: SGIs, the kernel timer
    // PPI, past the last entry and the special-ID range.
    for rejected in [0, config::TIMER_IRQ, last + 1, 1020] {
        assert_eq!(
            authorization::issue_line(0, node, 20, rejected),
            Err(INVALID_ARGUMENT),
            "line {rejected} must not be authorizable"
        );
    }
    let handler = authorization::issue_line(0, node, 20, first).expect("authorize slot 0 line");
    // A live line cannot be taken twice (seL4's RevokeFirst), and the
    // destination slot must be empty.
    assert_eq!(
        authorization::issue_line(0, node, 21, first),
        Err(REVOKE_FIRST)
    );
    assert_eq!(
        authorization::issue_line(0, node, 20, second),
        Err(ALREADY_MAPPED)
    );

    authorization::bind(handler, ntfn, 0x40).expect("bind notification");
    // Software-pend the line: delivery merges the binding badge into the
    // notification's pending bits.
    gic::set_pending(slot_line, true);
    synchronize();
    assert_eq!(interrupt::handle(), Outcome::Continue);
    assert_eq!(notification_bits(ntfn), 0x40, "IRQ badge delivered");

    // The line stays active after delivery: re-pending is held by the GIC,
    // not delivered — one driver pass per interrupt, no re-entry.
    gic::set_pending(slot_line, true);
    synchronize();
    assert_eq!(interrupt::handle(), Outcome::Continue);
    assert_eq!(
        notification_bits(ntfn),
        0x40,
        "active line must not redeliver"
    );

    // Ack deactivates the line; the latched pending fires on the next pass.
    // Drain the pending bits first (a driver's wait does the same): the merge
    // is idempotent, so the redelivery is only observable from a clean slate.
    authorization::acknowledge(handler).expect("ack");
    object::with_notification_bits(ntfn, |bits| *bits = 0).expect("drain");
    assert_eq!(interrupt::handle(), Outcome::Continue);
    assert_eq!(notification_bits(ntfn), 0x40, "redelivered after Ack");

    // Clear drops the binding and quiesces the line: silent even when pended.
    authorization::clear(handler).expect("clear");
    assert_eq!(authorization::delivery_target(first), None, "binding gone");
    gic::set_pending(slot_line, true);
    synchronize();
    assert_eq!(interrupt::handle(), Outcome::Continue);
    assert_eq!(
        notification_bits(ntfn),
        0x40,
        "cleared line must stay silent"
    );

    // Retire the handler exactly as collection would, so the real boot can
    // authorize the line again.
    gic::set_pending(slot_line, false);
    if let Some(Object::IrqHandler(handler)) = object::test_remove(handler) {
        authorization::retire(&handler);
    }
    object::test_remove(ntfn);
    object::test_remove(node);
    object::test_remove(control);
}
