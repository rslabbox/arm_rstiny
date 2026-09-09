//! AArch64 physical generic timer: one-shot hardware, no tick or GIC policy.
use super::{instructions, time};
use aarch64_cpu::{asm::barrier, registers::*};

/// Leave the local timer disabled until its consumer installs an IRQ route.
pub(crate) fn init() {
    assert!(time::frequency() != 0, "generic timer frequency is zero");
    stop();
}

/// Program an absolute deadline in system-counter ticks (the `time::now` domain).
/// An already elapsed deadline asserts the source immediately. CVAL avoids the
/// signed 32-bit interval limitation of TVAL. Local IRQs must be masked.
pub(crate) fn arm(deadline: u64) {
    assert!(instructions::irq_masked());
    CNTP_CVAL_EL0.set(deadline);
    CNTP_CTL_EL0.write(CNTP_CTL_EL0::ENABLE::SET + CNTP_CTL_EL0::IMASK::CLEAR);
    barrier::isb(barrier::SY);
}

/// Disable the local timer and deassert its interrupt source.
pub(crate) fn stop() {
    assert!(instructions::irq_masked());
    CNTP_CTL_EL0.write(CNTP_CTL_EL0::ENABLE::CLEAR + CNTP_CTL_EL0::IMASK::SET);
    barrier::isb(barrier::SY);
}
