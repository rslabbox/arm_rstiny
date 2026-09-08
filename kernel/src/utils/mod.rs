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

/// Select the PSCI conduit from the DTB supplied by elfloader.
/// Kernel entry is EL1 for both firmware configurations.
#[inline(never)]
#[unsafe(no_mangle)]
pub extern "C" fn kernel_shutdown() -> ! {
    const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;
    unsafe {
        core::arch::asm!("msr daifset, #0xf", options(nomem, nostack));
        if crate::arch::boot::psci_smc() {
            core::arch::asm!("smc #0",
                inlateout("x0") PSCI_SYSTEM_OFF => _,
                inlateout("x1") 0_u64 => _,
                inlateout("x2") 0_u64 => _,
                inlateout("x3") 0_u64 => _,
            );
        } else {
            core::arch::asm!("hvc #0",
                inlateout("x0") PSCI_SYSTEM_OFF => _,
                inlateout("x1") 0_u64 => _,
                inlateout("x2") 0_u64 => _,
                inlateout("x3") 0_u64 => _,
            );
        }
    }
    // Firmware should not return. Avoid recursively panicking if it does.
    halt()
}
pub use kernel_shutdown as shutdown;

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe { core::arch::asm!("msr daifset, #0xf", options(nomem, nostack)) };
    // A recursive panic must not recurse through formatting indefinitely.
    if !PANICKING.swap(true, Ordering::Relaxed) {
        console::panic_print(info);
    }
    shutdown()
}
