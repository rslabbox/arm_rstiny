//! Timer drivers.

pub mod generic_timer;

use core::sync::atomic::{AtomicU64, Ordering};

use arm_gic::IntId;
pub use generic_timer::*;
use provider_core::with_provider;

use crate::{
    TinyResult, config,
    device::core::{DeviceInfo, InitLevel},
    device::provider::IrqProvider,
};

// Setup timer interrupt handler
const PERIODIC_INTERVAL_NANOS: u64 = NANOS_PER_SEC / config::kernel::TICKS_PER_SEC as u64;
static NEXT_PERIODIC_DEADLINE: AtomicU64 = AtomicU64::new(0);

static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);

fn update_deadline(deadline_ns: u64) {
    NEXT_DEADLINE.store(deadline_ns, Ordering::Release);
    set_oneshot_timer(deadline_ns);
}

fn handle_timer_irq(_irq: usize) {
    let current_ns = current_nanoseconds();
    let mut next_deadline = NEXT_PERIODIC_DEADLINE.load(Ordering::Acquire);

    if current_ns >= next_deadline {
        NEXT_PERIODIC_DEADLINE.fetch_add(PERIODIC_INTERVAL_NANOS, Ordering::Release);
        next_deadline = NEXT_PERIODIC_DEADLINE.load(Ordering::Acquire);
    }

    update_deadline(next_deadline);
}

fn probe(_dev: &DeviceInfo) -> TinyResult<()> {
    // Initialize the generic timer early in the boot process
    generic_timer::init_early();
    // set Deadline
    let deadline = current_nanoseconds() + PERIODIC_INTERVAL_NANOS;
    NEXT_PERIODIC_DEADLINE.store(deadline, Ordering::Release);
    update_deadline(deadline);
    // Enable Timer interrupt
    with_provider::<IrqProvider>().register(config::kernel::TIMER_IRQ, handle_timer_irq);
    // Timer interrupt ID on ARM GIC
    enable_irqs(config::kernel::TIMER_IRQ);
    Ok(())
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

provider_core::define_provider!(
    provider: TIMER_PROVIDER,
    vendor_id: 0,
    device_id: 0,
    priority: 100,
    ops: crate::device::provider::TimerProvider {
        boot_nanoseconds,
        nanos_per_sec: || NANOS_PER_SEC,
        current_nanoseconds: generic_timer::current_nanoseconds,
        busy_wait,
        init_secondary,
    },
    driver: {
        name: "generic-timer",
        level: InitLevel::Early,
        compatibles: ["arm,armv8-timer", "arm,armv7-timer"],
        probe: probe,
    }
);
