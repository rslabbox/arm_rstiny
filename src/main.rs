#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

mod boot;

#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    panic!("Reached rust_main!");
    loop {}
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
