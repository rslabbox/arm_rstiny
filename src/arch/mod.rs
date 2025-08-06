mod context_frame;
mod trap;
mod boot;

use crate::config::PSCI_SYSTEM_OFF;
use crate::println;
use core::arch::global_asm;
use core::{arch::asm, panic::PanicInfo};
use log::warn;

global_asm!(include_str!("exception.s"));

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("PANIC: {}", info);
    system_shutdown();
}

#[inline]
pub fn system_shutdown() -> ! {
    warn!("Shutting down system...");
    unsafe {
        asm!("hvc #0", in("x0") PSCI_SYSTEM_OFF, in("x1") 0, in("x2") 0, in("x3") 0);
    }

    loop {
        core::hint::spin_loop();
    }
}
