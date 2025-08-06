#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![feature(alloc_error_handler)]
#![feature(get_mut_unchecked)]

use crate::drivers::misc::shutdown;

extern crate alloc;

#[macro_use]
extern crate log;

#[macro_use]
mod logging;

mod arch;
mod config;
mod drivers;
mod mm;
mod platform;
mod sync;
mod lang_items;
mod timer;
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

const LOGO: &str = r"
NN   NN  iii               bb        OOOOO    SSSSS
NNN  NN       mm mm mmmm   bb       OO   OO  SS
NN N NN  iii  mmm  mm  mm  bbbbbb   OO   OO   SSSSS
NN  NNN  iii  mmm  mm  mm  bb   bb  OO   OO       SS
NN   NN  iii  mmm  mm  mm  bbbbbb    OOOO0    SSSSS
              ___    ____    ___    ___
             |__ \  / __ \  |__ \  |__ \
             __/ / / / / /  __/ /  __/ /
            / __/ / /_/ /  / __/  / __/
           /____/ \____/  /____/ /____/
";

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    clear_bss();
    drivers::init_early();
    println!("{}", LOGO);

    mm::init_heap_early();
    logging::log_init();
    info!("Logging is enabled.");

    arch::init();
    arch::init_percpu();
    // percpu::init_percpu_early();

    mm::init();
    drivers::init();

    info!("Start OK");

    test::run_allocator_tests();

    shutdown();
}
