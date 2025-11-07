//! ARM Generic Timer driver.

use core::sync::atomic::{AtomicU64, Ordering};

use aarch64_cpu::registers::{CNTFRQ_EL0, CNTP_CTL_EL0, CNTP_TVAL_EL0, CNTPCT_EL0};
use aarch64_cpu::registers::{Readable, Writeable};
use arm_gic::IntId;
use int_ratio::Ratio;

use crate::drivers::irq::irqset_enable;

static mut CNTPCT_TO_NANOS_RATIO: Ratio = Ratio::zero();
static mut NANOS_TO_CNTPCT_RATIO: Ratio = Ratio::zero();
static BOOT_TICKS: AtomicU64 = AtomicU64::new(0);

/// Number of milliseconds in a second.
pub const MILLIS_PER_SEC: u64 = 1_000;
/// Number of microseconds in a second.
pub const MICROS_PER_SEC: u64 = 1_000_000;
/// Number of nanoseconds in a second.
pub const NANOS_PER_SEC: u64 = 1_000_000_000;
/// Number of nanoseconds in a millisecond.
pub const NANOS_PER_MILLIS: u64 = 1_000_000;
/// Number of nanoseconds in a microsecond.
pub const NANOS_PER_MICROS: u64 = 1_000;

/// Get current hardware timer ticks.
pub fn current_ticks() -> u64 {
    CNTPCT_EL0.get()
}

/// Get ticks since boot.
pub fn boot_ticks() -> u64 {
    current_ticks() - BOOT_TICKS.load(Ordering::Relaxed)
}

/// Converts hardware ticks to nanoseconds.
pub fn ticks_to_nanos(ticks: u64) -> u64 {
    unsafe { CNTPCT_TO_NANOS_RATIO.mul_trunc(ticks) }
}

/// Converts nanoseconds to hardware ticks.
pub fn nanos_to_ticks(nanos: u64) -> u64 {
    unsafe { NANOS_TO_CNTPCT_RATIO.mul_trunc(nanos) }
}

/// Set a one-shot timer.
///
/// A timer interrupt will be triggered at the specified monotonic time deadline (in nanoseconds).
pub fn set_oneshot_timer(deadline_ns: u64) {
    let cnptct = CNTPCT_EL0.get();
    let cnptct_deadline = nanos_to_ticks(deadline_ns);
    if cnptct < cnptct_deadline {
        let interval = cnptct_deadline - cnptct;
        debug_assert!(interval <= u32::MAX as u64);
        CNTP_TVAL_EL0.set(interval);
    } else {
        CNTP_TVAL_EL0.set(0);
    }
}

/// Early stage initialization: stores the timer frequency.
pub fn init_early() {
    let freq = CNTFRQ_EL0.get();
    unsafe {
        CNTPCT_TO_NANOS_RATIO = Ratio::new(NANOS_PER_SEC as u32, freq as u32);
        NANOS_TO_CNTPCT_RATIO = CNTPCT_TO_NANOS_RATIO.inverse();
    }
    BOOT_TICKS.store(current_ticks(), Ordering::Relaxed);
}

/// Enable timer interrupts.
///
/// It should be called on all CPUs, as the timer interrupt is a PPI (Private
/// Peripheral Interrupt).
pub fn enable_irqs(timer_irq_num: IntId) {
    CNTP_CTL_EL0.write(CNTP_CTL_EL0::ENABLE::SET);
    CNTP_TVAL_EL0.set(0);
    irqset_enable(timer_irq_num);
}
