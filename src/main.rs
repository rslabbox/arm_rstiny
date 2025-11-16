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

mod sync;
mod syscall;
mod task;

#[macro_use]
extern crate log;

extern crate alloc;

pub use error::{TinyError, TinyResult};
use memory_addr::pa;

use core::time::Duration;

use drivers::timer;

use crate::{drivers::timer::busy_wait, mm::phys_to_virt};

fn kernel_init() {
    hal::init_exception();
    hal::clear_bss();
    drivers::uart::init_early(phys_to_virt(pa!(platform::config::UART_PADDR)));
    timer::init_early();
    drivers::power::init("hvc").expect("Failed to initialize PSCI");
    drivers::irq::init().expect("Failed to initialize IRQ");

    // Print build time
    println!(
        "\n\nBuild time: {}",
        option_env!("BUILD_TIME").unwrap_or("unknown")
    );

    println!("Board: {}", platform::config::BOARD_NAME);

    console::init_logger().expect("Failed to initialize logger");
}

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    kernel_init();

    println!("\nHello RustTinyOS!\n");

    tests::rstiny_tests();

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
