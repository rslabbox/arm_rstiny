//! Timer drivers.

pub mod generic_timer;


#[allow(unused)]
pub use generic_timer::{
    NANOS_PER_MICROS, NANOS_PER_MILLIS, NANOS_PER_SEC, boot_ticks, current_ticks, enable_irqs,
    init_early, nanos_to_ticks, set_oneshot_timer, ticks_to_nanos,
};
