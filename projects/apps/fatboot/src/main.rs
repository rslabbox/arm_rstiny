#![no_std]
#![no_main]
use core::sync::atomic::{AtomicU64, Ordering};
use rstiny::{TaskState, debug_println, suspend_self};
use rstiny_runtime::{BootInfo, entry};

include!(concat!(env!("OUT_DIR"), "/hello.rs"));
unsafe extern "C" {
    static __hello_start: u8;
    static __hello_end: u8;
}

// Host-visible completion also verifies a silent LOG=off boot.
#[unsafe(no_mangle)]
static BOOTINFO_ADDRESS: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
static RESULT: AtomicU64 = AtomicU64::new(0);

#[entry(stack_size = 32 * 1024)]
fn main(info: &mut BootInfo) -> ! {
    BOOTINFO_ADDRESS.store(info.address() as u64, Ordering::Relaxed);
    debug_println!("[fatboot] loading hello.elf");
    // Linker-owned immutable resource; it contains a separate executable ELF.
    let image = unsafe {
        let start = core::ptr::addr_of!(__hello_start);
        core::slice::from_raw_parts(
            start,
            core::ptr::addr_of!(__hello_end) as usize - start as usize,
        )
    };
    let child = rstiny::elf::spawn(image).expect("load hello.elf");
    let code = child.wait().expect("wait hello");
    assert_eq!(child.status().expect("hello status"), TaskState::Exited);
    assert_eq!(code, 0, "hello failed");
    child.destroy().expect("reap hello");
    RESULT.store(1, Ordering::Relaxed);
    debug_println!("[fatboot] hello.elf exited successfully");
    suspend_self()
}
