#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

mod arch;
mod config;
mod utils;
mod test;

use utils::logging;

#[macro_use]
extern crate log;

extern crate alloc;

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    arch::arch_init();

    println!("\nHello RustTinyOS...");

    logging::log_init();
    info!("Reached rust_main!");
    error!("This is an error message for testing.");
    debug!("This is a debug message for testing.");
    trace!("This is a trace message for testing.");
    warn!("This is a warning message for testing.");
    
    test::run_allocator_tests();

    loop {
        
    }
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
