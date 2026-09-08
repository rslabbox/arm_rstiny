#![no_std]
#![no_main]

extern crate alloc;

mod arch;
mod config;
mod root_task;
#[cfg(feature = "kernel-test")]
mod test;
mod utils;

// Entry exception level retained for debugger inspection.
#[unsafe(no_mangle)]
static mut BOOT_ENTRY_EL_VALUE: u64 = 0;
const BOOT_ENTRY_EL: *mut u64 = core::ptr::addr_of_mut!(BOOT_ENTRY_EL_VALUE);

pub fn rust_main() -> ! {
    utils::logging::init();
    utils::heap_allocator::init_heap();
    log::info!("ARM RSTiny: EL1, MMU on");
    #[cfg(feature = "kernel-test")]
    test::run();
    log::info!("Kernel ready: launching fatboot");
    root_task::start_root()
}
