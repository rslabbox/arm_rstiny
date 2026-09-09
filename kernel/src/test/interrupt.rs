//! Real GIC/timer lifecycle checks, before scheduler tick initialization.
use crate::{
    arch::machine::{
        gic::{self, IrqId, Trigger},
        time, timer,
    },
    interrupt::{self, Outcome},
};
use aarch64_cpu::{asm::barrier, registers::*};

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
    gic::complete(active);
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
