pub mod heap_allocator;
pub mod logging;
mod timer;

mod console;
use core::arch::asm;
use core::panic::PanicInfo;
pub use timer::current_ticks;

const PSCI_SYSTEM_OFF: u32 = 0x8400_0008;

fn psci_hvc_call(func: u32, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let ret;
    unsafe {
        asm!(
            "hvc #0",
            inlateout("x0") func as usize => ret,
            in("x1") arg0,
            in("x2") arg1,
            in("x3") arg2,
        )
    }
    ret
}

pub fn shutdown() -> ! {
    warn!("Shutting down...");
    psci_hvc_call(PSCI_SYSTEM_OFF, 0, 0, 0);
    unreachable!("It should shutdown!")
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        error!(
            "Panicked at {}:{} {}",
            location.file(),
            location.line(),
            info.message()
        );
    } else {
        error!("Panicked: {}", info.message());
    }

    shutdown()
}
