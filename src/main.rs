//! RstinyOS - Main kernel entry point.

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

mod boot;
mod config;

mod console;
mod error;

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

pub use error::{TinyError, TinyResult};

use core::time::Duration;

use drivers::timer;

use crate::drivers::timer::busy_wait;


fn kernel_init() {
    hal::init_exception();
    hal::clear_bss();
    drivers::power::init("hvc").expect("Failed to initialize PSCI");
    drivers::irq::init().expect("Failed to initialize IRQ");
    timer::init_early();
}

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    kernel_init();

    // Print build time
    println!(
        "Build time: {}",
        option_env!("BUILD_TIME").unwrap_or("unknown")
    );

    println!("Board: {}", platform::config::BOARD_NAME);

    println!("\nHello RustTinyOS!\n");

    console::init_logger().expect("Failed to initialize logger");
    info!("This is an info message for testing.");
    error!("This is an error message for testing.");
    debug!("This is a debug message for testing.");
    trace!("This is a trace message for testing.");
    warn!("This is a warning message for testing.");

    tests::run_allocator_tests();

    drivers::pci::test_dw_pcie_atu();

    loop {
        busy_wait(Duration::from_secs(1));
    }
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use drivers::power::system_off;

    println!("PANIC: {}", info);
    system_off();
}
