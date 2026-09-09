#![no_std]
#![no_main]

extern crate alloc;

mod api;
mod arch;
mod boot;
mod config;
mod interrupt;
mod memory;
mod object;
mod task;
#[cfg(feature = "kernel-test")]
mod test;
mod utils;

pub fn rust_main() -> ! {
    utils::logging::init();
    utils::heap_allocator::init_heap();
    log::info!("ARM RSTiny: EL1, MMU on");
    #[cfg(feature = "kernel-test")]
    test::run();
    log::info!("Kernel ready: launching fatboot");
    boot::start_root()
}
