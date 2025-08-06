#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;
use log::info;

mod allocator;
mod config;
mod console;
mod arch;

mod test;

#[unsafe(no_mangle)]
pub extern "C" fn rust_main() -> ! {
    // 初始化控制台和日志系统
    console::log_init();

    // 初始化堆分配器
    allocator::init_heap();

    // 问候语
    info!("ARM RSTiny - Rust Bare Metal OS");

    test::run_allocator_tests();

    arch::system_shutdown();
}
