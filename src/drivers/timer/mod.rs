//! Timer drivers.

pub mod generic_timer;
pub mod timer_lists;

use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use arm_gic::IntId;
pub use generic_timer::*;
use kspin::SpinNoIrq;

use crate::{
    config,
    drivers::{self},
};
use timer_lists::TimerList;

// Setup timer interrupt handler
const PERIODIC_INTERVAL_NANOS: u64 = NANOS_PER_SEC / config::kernel::TICKS_PER_SEC as u64;
static NEXT_PERIODIC_DEADLINE: AtomicU64 = AtomicU64::new(0);

lazy_static::lazy_static! {
    static ref TIMER_LIST: SpinNoIrq<TimerList> = SpinNoIrq::new(TimerList::new());
}

static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);

fn update_deadline(deadline_ns: u64) {
    NEXT_DEADLINE.store(deadline_ns, Ordering::Release);
    set_oneshot_timer(deadline_ns);
}

/// Set a timer with a callback to be executed at the specified duration.
pub fn set_timer(duration: Duration, callback: impl FnOnce(u64) + Send + Sync + 'static) {
    let deadline_ns = duration.as_nanos() as u64 + current_nanoseconds();
    TIMER_LIST.lock().set(deadline_ns, callback);
    if deadline_ns < NEXT_DEADLINE.load(Ordering::Acquire) {
        update_deadline(deadline_ns);
    }
}

fn handle_timer_irq(_irq: usize) {
    let current_ns = current_nanoseconds();
    let mut next_deadline = NEXT_PERIODIC_DEADLINE.load(Ordering::Acquire);

    if current_ns >= next_deadline {
        NEXT_PERIODIC_DEADLINE.fetch_add(PERIODIC_INTERVAL_NANOS, Ordering::Release);
        next_deadline = NEXT_PERIODIC_DEADLINE.load(Ordering::Acquire);
    }

    let mut timers = TIMER_LIST.lock();
    while timers.expire_one(current_nanoseconds()).is_some() {}

    if let Some(d) = timers.next_deadline() {
        next_deadline = next_deadline.min(d);
    }

    update_deadline(next_deadline);
}

pub fn init_early() {
    // Initialize the generic timer early in the boot process
    generic_timer::init_early();
    // set Deadline
    let deadline = current_nanoseconds() + PERIODIC_INTERVAL_NANOS;
    NEXT_PERIODIC_DEADLINE.store(deadline, Ordering::Release);
    update_deadline(deadline);
    // Enable Timer interrupt
    drivers::irq::irqset_register(IntId::ppi(14), handle_timer_irq);
    // Timer interrupt ID on ARM GIC
    enable_irqs(IntId::ppi(14));
}

/// Initialize timer for secondary CPU.
///
/// Timer interrupt is a PPI (Private Peripheral Interrupt), so each CPU
/// needs to enable it independently.
pub fn init_secondary() {
    // Enable timer interrupt on this CPU
    // Note: The timer IRQ handler is already registered by primary CPU
    enable_irqs(IntId::ppi(14));
}
