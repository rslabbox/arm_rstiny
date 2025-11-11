//! Timer drivers.

pub mod generic_timer;
pub mod smoltcp_time;

use core::sync::atomic::{AtomicU64, Ordering};

use arm_gic::IntId;
pub use generic_timer::*;

use crate::{config, drivers};

// Setup timer interrupt handler
const PERIODIC_INTERVAL_NANOS: u64 = NANOS_PER_SEC / config::kernel::TICKS_PER_SEC as u64;

static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);

fn update_timer(_irq: usize) {
    let current_ns = ticks_to_nanos(current_ticks());
    let mut deadline = NEXT_DEADLINE.load(Ordering::Relaxed);
    if current_ns >= deadline {
        deadline = current_ns + PERIODIC_INTERVAL_NANOS;
    }
    // Set the next timer deadline (1 second later)
    let next_deadline_ns = deadline + NANOS_PER_SEC;
    NEXT_DEADLINE.store(next_deadline_ns, Ordering::Relaxed);
    set_oneshot_timer(next_deadline_ns);
}

pub fn init_early() {
    generic_timer::init_early();

    // Enable Timer interrupt
    drivers::irq::irqset_register(IntId::ppi(14), update_timer);
    // Timer interrupt ID on ARM GIC
    enable_irqs(IntId::ppi(14)); 
}
