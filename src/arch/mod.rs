mod context_frame;
mod trap;

use crate::config::PSCI_SYSTEM_OFF;
use core::arch::global_asm;
use core::{arch::asm, panic::PanicInfo};
use log::{error, warn};

global_asm!(include_str!("exception.s"));
global_asm!(include_str!("boot.s"));

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    error!("PANIC: {}", info);
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
