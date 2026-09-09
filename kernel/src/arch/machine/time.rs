//! Monotonic system clock from the AArch64 generic timer's system counter.
//!
//! Reading the counter needs no driver state and no interrupt controller, so
//! these helpers are callable from any IRQ-masked kernel path: scheduler
//! deadlines, managed-runtime timeouts and clock requests all share this one
//! time base (counter ticks at [frequency] Hz).
use aarch64_cpu::registers::{CNTFRQ_EL0, CNTPCT_EL0, Readable};

/// Current system-counter value, in ticks since an arbitrary epoch.
#[inline]
pub fn now() -> u64 {
    CNTPCT_EL0.get()
}

/// System-counter frequency in Hz, as programmed by the boot firmware.
#[inline]
pub fn frequency() -> u64 {
    CNTFRQ_EL0.get()
}
