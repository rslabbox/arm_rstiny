#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

mod arch;
mod config;
mod utils;
mod test;
mod driver;
mod net;

use utils::logging;

#[macro_use]
extern crate log;

extern crate alloc;

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    arch::arch_init();

    // 打印编译时间
    println!("Build time: {}", option_env!("BUILD_TIME").unwrap_or("unknown"));

    println!("\nHello RustTinyOS!\n");

    logging::log_init();
    info!("This is an info message for testing.");
    error!("This is an error message for testing.");
    debug!("This is a debug message for testing.");
    trace!("This is a trace message for testing.");
    warn!("This is a warning message for testing.");
    
    test::run_allocator_tests();

    driver::probe_mmio_device();

    loop {
        
    }
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use crate::arch::device::psci::system_off;

    println!("PANIC: {}", info);
    system_off();
}
