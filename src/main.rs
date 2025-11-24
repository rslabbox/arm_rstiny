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
mod task;
mod tests;

// Future modules (placeholder)
mod fs;

#[cfg(feature = "net")]
mod net;
mod sync;
mod syscall;

#[macro_use]
extern crate log;

extern crate alloc;

pub use error::{TinyError, TinyResult};
use memory_addr::pa;

use crate::mm::phys_to_virt;
use drivers::timer;

fn kernel_init() {
    hal::clear_bss();
    hal::init_exception();
    drivers::uart::init_early(phys_to_virt(pa!(config::UART_PADDR)));

    // Print build time
    println!(
        "\n\nBuild time: {}",
        option_env!("BUILD_TIME").unwrap_or("unknown")
    );

    println!("Board: {}", config::BOARD_NAME);

    console::init_logger().expect("Failed to initialize logger");
    drivers::irq::init(
        phys_to_virt(pa!(config::GICD_BASE)),
        phys_to_virt(pa!(config::GICR_BASE)),
    )
    .expect("Failed to initialize IRQ");

    timer::init_early();
    drivers::power::init("hvc").expect("Failed to initialize PSCI");
    
    // Initialize task scheduler
    task::init_scheduler();
}

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    kernel_init();

    println!("\nHello RustTinyOS!\n");

    tests::rstiny_tests();

    drivers::power::system_off();
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("PANIC: {}", info);
    drivers::power::system_off();
}
