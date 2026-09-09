#![no_std]
#![no_main]

mod archive;
mod boot_info;
mod console;
mod device_tree;
mod elf;
mod entry;
mod handoff;
mod image;
mod layout;
mod mmu;
mod pl011;

mod platform;

use console::bootinfo;

/// Runs after the assembly entry has established the stack and cleared BSS.
fn boot_main() -> ! {
    console::init();
    bootinfo!("Rust bootloader started (AArch64 EL1)");
    match image::plan() {
        // SAFETY: Entry established the single-core EL1 environment;
        // the plan validated every destination before load writes physical RAM.
        Ok(plan) => unsafe { handoff::enter(plan.load()) },
        Err(error) => console::fail(error),
    }
}
