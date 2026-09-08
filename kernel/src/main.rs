#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

mod arch;
mod config;
#[cfg(feature = "kernel-test")]
mod test;
mod utils;

// Stable debugger-visible status, independent of UART and log configuration.
// 0 = reset, 1 = boot tables, 2 = MMU on, 3 = Kernel ready,
// 0xe1 = exception, 0xe2 = panic.
#[unsafe(no_mangle)]
static mut BOOT_STATE: u64 = 0;
#[unsafe(no_mangle)]
static mut BOOT_ENTRY_EL_VALUE: u64 = 0;
const BOOT_ENTRY_EL: *mut u64 = core::ptr::addr_of_mut!(BOOT_ENTRY_EL_VALUE);

unsafe fn set_boot_state(state: u64) {
    unsafe { core::ptr::addr_of_mut!(BOOT_STATE).write_volatile(state) };
}

pub fn rust_main() -> ! {
    utils::logging::init();
    utils::heap_allocator::init_heap();
    log::info!("ARM RSTiny: EL1, MMU on, kernel only");
    #[cfg(feature = "kernel-test")]
    test::run();
    log::info!("Kernel ready: interrupts masked; parked");
    unsafe { set_boot_state(3) };
    kernel_idle()
}

/// The kernel has no scheduler or enabled interrupt sources. This is a parked CPU,
/// not yet a scheduler idle thread. A debugger can interrupt WFE.
#[unsafe(naked)]
#[unsafe(no_mangle)]
extern "C" fn kernel_idle() -> ! {
    core::arch::naked_asm!("msr daifset, #0xf", "2:", "wfe", "b 2b");
}
