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
extern crate axbacktrace;

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
        "\nBuild time: {}",
        option_env!("BUILD_TIME").unwrap_or("unknown")
    );

    println!("Board: {}", config::BOARD_NAME);

    console::init_logger().expect("Failed to initialize logger");

    use core::ops::Range;

    unsafe extern "C" {
        safe static _stext: [u8; 0];
        safe static _etext: [u8; 0];
        safe static _edata: [u8; 0];
    }

    let ip_range = Range {
        start: _stext.as_ptr() as usize,
        end: _etext.as_ptr() as usize,
    };

    let fp_range = Range {
        start: _edata.as_ptr() as usize,
        end: usize::MAX,
    };
    info!("ip_range: {:#x} - {:#x}", ip_range.start, ip_range.end);
    info!("fp_range: {:#x} - {:#x}", fp_range.start, fp_range.end);
    axbacktrace::init(ip_range, fp_range);

    drivers::irq::init(
        phys_to_virt(pa!(config::GICD_BASE)),
        phys_to_virt(pa!(config::GICR_BASE)),
    )
    .expect("Failed to initialize IRQ");

    timer::init_early();
    drivers::power::init("hvc").expect("Failed to initialize PSCI");

    // Initialize task scheduler
    task::init_taskmanager();
}

/// User main task entry point.
fn user_main() {
    // Run tests in main task
    tests::rstiny_tests();

    // Run scheduler tests
    task::tests::run_scheduler_tests();

    info!("User main task completed");
}

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    kernel_init();

    println!("\nHello RustTinyOS!\n");

    // Create main user task as child of ROOT
    task::spawn_main_task(user_main);

    // Start scheduler, transfer control to ROOT
    task::start_scheduling();
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    error!("{}", info);

    // Capture and display backtrace
    error!("\n{}", axbacktrace::Backtrace::capture());

    drivers::power::system_off();
}
