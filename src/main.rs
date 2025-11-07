//! RstinyOS - Main kernel entry point.

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

mod boot;
mod config;

mod console;

mod drivers;
mod hal;
mod mm;
mod platform;
mod tests;

// Future modules (placeholder)
mod fs;
mod net;
mod sync;
mod syscall;
mod task;

#[macro_use]
extern crate log;

extern crate alloc;

use arm_gic::IntId;
use core::sync::atomic::{AtomicU64, Ordering};

use drivers::timer;

// Setup timer interrupt handler
const PERIODIC_INTERVAL_NANOS: u64 = timer::NANOS_PER_SEC / config::kernel::TICKS_PER_SEC as u64;

static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);

fn update_timer(_irq: usize) {
    let current_ns = timer::ticks_to_nanos(timer::current_ticks());
    let mut deadline = NEXT_DEADLINE.load(Ordering::Relaxed);
    if current_ns >= deadline {
        deadline = current_ns + PERIODIC_INTERVAL_NANOS;
    }
    // Set the next timer deadline (1 second later)
    let next_deadline_ns = deadline + timer::NANOS_PER_SEC;
    NEXT_DEADLINE.store(next_deadline_ns, Ordering::Relaxed);
    timer::set_oneshot_timer(next_deadline_ns);
}

fn kernel_init() {
    hal::init_exception();
    hal::clear_bss();
    timer::init_early();
    drivers::power::init("hvc");
    drivers::irq::init();

    // Enable Timer interrupt
    drivers::irq::irqset_register(IntId::ppi(14), update_timer);
    timer::enable_irqs(IntId::ppi(14));
}

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    kernel_init();

    // Print build time
    println!(
        "Build time: {}",
        option_env!("BUILD_TIME").unwrap_or("unknown")
    );

    println!("\nHello RustTinyOS!\n");

    console::init_logger();
    info!("This is an info message for testing.");
    error!("This is an error message for testing.");
    debug!("This is a debug message for testing.");
    trace!("This is a trace message for testing.");
    warn!("This is a warning message for testing.");

    tests::run_allocator_tests();

    loop {}
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use drivers::power::system_off;

    println!("PANIC: {}", info);
    system_off();
}
