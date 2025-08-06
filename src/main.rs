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
    info!("Console and logging initialized");

    // 初始化堆分配器
    allocator::init_heap();
    info!("Heap allocator initialized");

    // 问候语
    info!("ARM RSTiny - Rust Bare Metal OS");
    info!("Starting allocator tests...");

    test::run_allocator_tests();

    info!("All tests completed, shutting down...");
    arch::system_shutdown();
}
