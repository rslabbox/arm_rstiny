#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![feature(alloc_error_handler)]
#![feature(get_mut_unchecked)]

use crate::utils::shutdown;

extern crate alloc;

#[macro_use]
extern crate log;

#[macro_use]
mod logging;

mod arch;
mod config;
mod mm;
mod platform;
mod lang_items;
mod utils;
mod test;

fn clear_bss() {
    unsafe extern "C" {
        unsafe fn sbss();
        unsafe fn ebss();
    }
    unsafe {
        core::slice::from_raw_parts_mut(sbss as usize as *mut u8, ebss as usize - sbss as usize)
            .fill(0);
    }
}

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();

    mm::heap_allocator::init_heap();
    logging::log_init();
    info!("Logging is enabled.");

    arch::trap::init();

    info!("Start OK");

    test::run_allocator_tests();

    shutdown();
}
