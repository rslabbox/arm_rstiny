#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;
use log::info;

mod allocator;
mod console;
mod config;
mod utils;


#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    console::log_init();
    allocator::init_heap();
    info!("ARM RSTiny - Rust Bare Metal OS");

    utils::system_shutdown();
}

