use aarch64_cpu::registers::{CNTPCT_EL0, Readable};

pub fn current_ticks() -> u64 {
    CNTPCT_EL0.get()
}
