mod console;
pub mod heap_allocator;
pub mod logging;

use core::{
    panic::PanicInfo,
    sync::atomic::{AtomicBool, Ordering},
};

static PANICKING: AtomicBool = AtomicBool::new(false);

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_halt() -> ! {
    core::arch::naked_asm!("msr daifset, #0xf", "2:", "wfe", "b 2b");
}
pub use kernel_halt as halt;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe { core::arch::asm!("msr daifset, #0xf", options(nomem, nostack)) };
    unsafe { crate::set_boot_state(0xe2) };
    // A recursive panic must not recurse through formatting indefinitely.
    if !PANICKING.swap(true, Ordering::Relaxed) {
        console::panic_print(info);
    }
    halt()
}
