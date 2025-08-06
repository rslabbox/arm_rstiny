// use crate::drivers::misc::shutdown;
use core::panic::PanicInfo;

use crate::utils::shutdown;

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
