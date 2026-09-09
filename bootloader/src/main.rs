#![no_std]
#![no_main]

mod arch;
mod console;
mod image;
mod loader;
mod memory;
mod platform;

use console::bootinfo;

/// Runs after the assembly entry has established the stack and cleared BSS.
fn boot_main() -> ! {
    console::init();
    bootinfo!("Rust bootloader started (AArch64 EL1)");
    match loader::plan() {
        // SAFETY: Entry established the single-core EL1 environment;
        // the plan validated every destination before load writes physical RAM.
        Ok(plan) => unsafe { arch::enter(plan.load()) },
        Err(error) => console::fail(error),
    }
}
